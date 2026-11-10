//! The append-only ledger: one JSONL file of things and head changes, with
//! in-memory indexes rebuilt on open. Slot versions are counted in the same pass.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::id::{normalize_parents, Id, ThingId};
use crate::thing::{MadeBy, Payload, Thing};

/// The only mutable state. Two kinds, each with a version that goes up when it changes.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Slot {
    Head(String),      // version = number of times this head was set
    Children(ThingId), // version = number of distinct children appended
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Record {
    Thing(Thing),
    Head { name: String, id: ThingId, at: u64 },
}

/// What an append did.
#[derive(Debug, Clone, Default)]
pub struct Appended {
    /// False when the thing already existed (the append was a no-op).
    pub new: bool,
    /// Slots whose version went up.
    pub bumped: Vec<Slot>,
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Default)]
struct State {
    things: HashMap<ThingId, Thing>,
    order: Vec<ThingId>,
    children: HashMap<ThingId, BTreeSet<ThingId>>,
    heads: BTreeMap<String, ThingId>,
    versions: HashMap<Slot, u64>,
}

impl State {
    fn apply_thing(&mut self, thing: Thing) -> Appended {
        if self.things.contains_key(&thing.id) {
            return Appended::default();
        }
        let mut bumped = Vec::new();
        for p in &thing.parents {
            self.children.entry(*p).or_default().insert(thing.id);
            let slot = Slot::Children(*p);
            *self.versions.entry(slot.clone()).or_insert(0) += 1;
            bumped.push(slot);
        }
        self.order.push(thing.id);
        self.things.insert(thing.id, thing);
        Appended { new: true, bumped }
    }

    fn apply_head(&mut self, name: &str, id: ThingId) -> u64 {
        self.heads.insert(name.to_string(), id);
        let v = self.versions.entry(Slot::Head(name.to_string())).or_insert(0);
        *v += 1;
        *v
    }
}

pub struct Ledger {
    dir: PathBuf,
    // One writer: every append goes through this mutex and is written before
    // the call returns.
    inner: Mutex<Inner>,
}

struct Inner {
    state: State,
    file: File,
}

pub const LEDGER_FILE: &str = "ledger.jsonl";

impl Ledger {
    /// Open (or create) the ledger in `dir`, replaying the log to rebuild the indexes.
    pub fn open(dir: &Path) -> Result<Ledger> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(LEDGER_FILE);
        let records: Vec<Record> = read_jsonl(&path)?;
        let mut state = State::default();
        for r in records {
            match r {
                Record::Thing(t) => {
                    let expect = ThingId::compute(&t.kind, &t.payload, &t.parents);
                    if expect != t.id {
                        return Err(Error::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("{}: id {} does not match its content", path.display(), t.id),
                        )));
                    }
                    state.apply_thing(t);
                }
                Record::Head { name, id, .. } => {
                    state.apply_head(&name, id);
                }
            }
        }
        let file = open_append(&path)?;
        Ok(Ledger {
            dir: dir.to_path_buf(),
            inner: Mutex::new(Inner { state, file }),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("ledger mutex poisoned")
    }

    /// Append a thing. A thing whose id is already present is not written again.
    pub fn append(&self, thing: Thing) -> Result<Appended> {
        let mut g = self.lock();
        if g.state.things.contains_key(&thing.id) {
            return Ok(Appended::default());
        }
        for p in &thing.parents {
            if !g.state.things.contains_key(p) {
                return Err(Error::NotFound(format!("parent {p}")));
            }
        }
        write_line(&mut g.file, &Record::Thing(thing.clone()))?;
        Ok(g.state.apply_thing(thing))
    }

    /// Build and append a typed payload. Returns its id and what the append did.
    pub fn put<T: Payload>(
        &self,
        payload: &T,
        parents: &[ThingId],
        made_by: MadeBy,
    ) -> Result<(Id<T>, Appended)> {
        let value = serde_json::to_value(payload)?;
        let parents = normalize_parents(parents);
        let id = ThingId::compute(T::KIND, &value, &parents);
        let thing = Thing {
            id,
            kind: T::KIND.to_string(),
            payload: value,
            parents,
            made_by,
            at: now_millis(),
        };
        let appended = self.append(thing)?;
        Ok((Id::from_raw(id), appended))
    }

    /// Point a head at a thing. Returns the head's new version.
    pub fn set_head(&self, name: &str, id: ThingId) -> Result<u64> {
        let mut g = self.lock();
        if !g.state.things.contains_key(&id) {
            return Err(Error::NotFound(format!("thing {id}")));
        }
        write_line(
            &mut g.file,
            &Record::Head {
                name: name.to_string(),
                id,
                at: now_millis(),
            },
        )?;
        Ok(g.state.apply_head(name, id))
    }

    pub fn get(&self, id: &ThingId) -> Option<Thing> {
        self.lock().state.things.get(id).cloned()
    }

    pub fn contains(&self, id: &ThingId) -> bool {
        self.lock().state.things.contains_key(id)
    }

    /// Load and decode a typed payload.
    pub fn get_typed<T: Payload>(&self, id: Id<T>) -> Result<T> {
        let thing = self
            .get(&id.raw())
            .ok_or_else(|| Error::NotFound(format!("thing {}", id.raw())))?;
        thing.decode()
    }

    /// Children of `id`, sorted by id, with the version of `Slot::Children(id)`
    /// read in the same step.
    pub fn children_with_version(&self, id: &ThingId) -> (Vec<ThingId>, u64) {
        let g = self.lock();
        let kids = g
            .state
            .children
            .get(id)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default();
        let v = g
            .state
            .versions
            .get(&Slot::Children(*id))
            .copied()
            .unwrap_or(0);
        (kids, v)
    }

    pub fn children(&self, id: &ThingId) -> Vec<ThingId> {
        self.children_with_version(id).0
    }

    /// A head's target with the version of `Slot::Head(name)` read in the same step.
    pub fn head_with_version(&self, name: &str) -> (Option<ThingId>, u64) {
        let g = self.lock();
        let v = g
            .state
            .versions
            .get(&Slot::Head(name.to_string()))
            .copied()
            .unwrap_or(0);
        (g.state.heads.get(name).copied(), v)
    }

    pub fn head(&self, name: &str) -> Option<ThingId> {
        self.head_with_version(name).0
    }

    pub fn version(&self, slot: &Slot) -> u64 {
        self.lock().state.versions.get(slot).copied().unwrap_or(0)
    }

    /// All heads, sorted by name.
    pub fn heads(&self) -> Vec<(String, ThingId)> {
        self.lock()
            .state
            .heads
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    }

    /// Every thing in the order it was appended (oldest first).
    pub fn things(&self) -> Vec<Thing> {
        let g = self.lock();
        g.state
            .order
            .iter()
            .filter_map(|id| g.state.things.get(id).cloned())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.lock().state.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Resolve a full id or a unique prefix of at least 8 hex characters.
    pub fn resolve_prefix(&self, s: &str) -> Result<ThingId> {
        let s = s.trim().to_ascii_lowercase();
        if s.len() == 64 {
            let id: ThingId = s.parse().map_err(Error::NotFound)?;
            return if self.contains(&id) {
                Ok(id)
            } else {
                Err(Error::NotFound(format!("thing {s}")))
            };
        }
        if s.len() < 8 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::NotFound(format!(
                "{s:?} is not an id: give at least 8 hex characters"
            )));
        }
        let g = self.lock();
        let mut hits = g.state.order.iter().filter(|id| id.hex().starts_with(&s));
        match (hits.next(), hits.next()) {
            (Some(id), None) => Ok(*id),
            (None, _) => Err(Error::NotFound(format!("no thing starts with {s}"))),
            (Some(_), Some(_)) => Err(Error::NotFound(format!("prefix {s} is ambiguous"))),
        }
    }
}

fn open_append(path: &Path) -> Result<File> {
    Ok(OpenOptions::new().create(true).append(true).open(path)?)
}

fn write_line<T: Serialize>(file: &mut File, record: &T) -> Result<()> {
    let mut line = serde_json::to_vec(record)?;
    line.push(b'\n');
    // One write call per record, so a crash leaves at most one torn line.
    file.write_all(&line)?;
    file.flush()?;
    Ok(())
}

/// Read a JSONL file. A final line that does not parse is ignored (and cut off
/// the file, so later appends start on a clean line); any other bad line is an error.
pub(crate) fn read_jsonl<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    let mut offset = 0usize; // byte offset of the start of the current line
    let mut good_end = 0usize; // byte offset just past the last good line
    let lines: Vec<&str> = text.split('\n').collect();
    let last_idx = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .unwrap_or(0);
    let mut torn_tail = false;
    for (i, line) in lines.iter().enumerate() {
        let next = offset + line.len() + 1;
        if line.trim().is_empty() {
            offset = next;
            continue;
        }
        match serde_json::from_str::<T>(line) {
            Ok(v) => {
                out.push(v);
                good_end = next.min(text.len());
            }
            Err(e) if i == last_idx => {
                eprintln!(
                    "warning: ignoring corrupt last line of {} ({e})",
                    path.display()
                );
                torn_tail = true;
            }
            Err(e) => {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{}: line {}: {e}", path.display(), i + 1),
                )));
            }
        }
        offset = next;
    }
    if torn_tail {
        // DECISION: cut the torn tail off the file. Leaving it would glue the
        // next append onto the broken line and turn it into a bad middle line.
        let f = OpenOptions::new().write(true).open(path)?;
        f.set_len(good_end as u64)?;
    } else if !text.is_empty() && !text.ends_with('\n') {
        // A complete last record that lost only its newline: restore it.
        let mut f = OpenOptions::new().append(true).open(path)?;
        f.write_all(b"\n")?;
    }
    Ok(out)
}

/// Open a file for appending JSONL, creating it if needed.
pub(crate) fn open_jsonl_writer(path: &Path) -> Result<File> {
    open_append(path)
}

pub(crate) fn write_jsonl_line<T: Serialize>(file: &mut File, record: &T) -> Result<()> {
    write_line(file, record)
}
