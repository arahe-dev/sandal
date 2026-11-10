//! Memo entries: a result stored with the slots it read and their versions.

use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::id::{canonical_json, sha256};
use crate::ledger::{open_jsonl_writer, read_jsonl, write_jsonl_line, Ledger, Slot};
use crate::thing::Effect;

pub const MEMO_FILE: &str = "memo.jsonl";

mod key_hex {
    use serde::{de, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(k: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(k))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let s = String::deserialize(d)?;
        let bytes = hex::decode(&s).map_err(de::Error::custom)?;
        bytes
            .try_into()
            .map_err(|_| de::Error::custom("memo key must be 32 bytes"))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemoEntry {
    /// sha256(op name, op version, model id if Model else "", canonical JSON of input)
    #[serde(with = "key_hex")]
    pub key: [u8; 32],
    pub op: String,
    pub effect: Effect,
    pub input: Value,
    pub output: Value,
    /// Every slot read, by this op or any op it called.
    pub deps: Vec<(Slot, u64)>,
    pub at: u64,
}

impl MemoEntry {
    /// True when every recorded slot version still equals the current one.
    pub fn is_valid(&self, ledger: &Ledger) -> bool {
        self.deps.iter().all(|(slot, v)| ledger.version(slot) == *v)
    }
}

pub fn memo_key(op: &str, version: u32, model_id: &str, input: &Value) -> [u8; 32] {
    sha256(&[
        op.as_bytes(),
        &version.to_be_bytes(),
        model_id.as_bytes(),
        &canonical_json(input),
    ])
}

/// All memo entries, loaded from `memo.jsonl` (the last entry for a key wins)
/// and appended to as new results are computed.
pub(crate) struct MemoStore {
    inner: Mutex<MemoInner>,
}

struct MemoInner {
    map: HashMap<[u8; 32], MemoEntry>,
    file: File,
}

impl MemoStore {
    pub(crate) fn open(dir: &Path) -> Result<MemoStore> {
        let path = dir.join(MEMO_FILE);
        let entries: Vec<MemoEntry> = read_jsonl(&path)?;
        let map = entries.into_iter().map(|e| (e.key, e)).collect();
        Ok(MemoStore {
            inner: Mutex::new(MemoInner {
                map,
                file: open_jsonl_writer(&path)?,
            }),
        })
    }

    pub(crate) fn get(&self, key: &[u8; 32]) -> Option<MemoEntry> {
        self.inner.lock().unwrap().map.get(key).cloned()
    }

    pub(crate) fn put(&self, entry: MemoEntry) -> Result<()> {
        let mut g = self.inner.lock().unwrap();
        write_jsonl_line(&mut g.file, &entry)?;
        g.map.insert(entry.key, entry);
        Ok(())
    }

    pub(crate) fn all(&self) -> Vec<MemoEntry> {
        self.inner.lock().unwrap().map.values().cloned().collect()
    }
}
