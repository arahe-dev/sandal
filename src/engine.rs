//! The engine runs an operation on demand, returns a memoised result when its
//! recorded dependencies are unchanged, and recomputes only what new evidence
//! touched. It is hand-written: model calls are async and the rules are small.

use std::path::Path;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::{Semaphore, SemaphorePermit};

use crate::ctx::{Ctx, Frame};
use crate::error::{Error, Result};
use crate::fx::Fx;
use crate::id::{Id, ThingId};
use crate::ledger::{now_millis, Ledger};
use crate::memo::{memo_key, MemoEntry, MemoStore};
use crate::op::Op;
use crate::plug::{Model, Source, Verifier};
use crate::thing::{Effect, Map, MadeBy, Payload, Proposal, Thing};
use crate::verify::Verified;

#[derive(Clone, Debug)]
pub struct Budget {
    pub max_model_calls: u64,
    pub max_world_calls: u64,
    pub max_concurrency: usize, // one shared tokio Semaphore
}

impl Default for Budget {
    fn default() -> Self {
        Budget {
            max_model_calls: 200,
            max_world_calls: 50,
            max_concurrency: 8,
        }
    }
}

pub struct Plugs {
    pub model: Arc<dyn Model>,
    pub sources: Vec<Arc<dyn Source>>,
    pub verifier: Arc<dyn Verifier>,
}

/// Counters for this run of the engine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub model_calls: u64,
    pub world_calls: u64,
    pub hits: u64,
    pub misses: u64,
}

pub const DEFAULT_MAX_TOKENS: u32 = 2000;

/// State shared by the engine and every `Ctx`.
pub(crate) struct Core {
    pub(crate) ledger: Ledger,
    pub(crate) memo: MemoStore,
    pub(crate) plugs: Plugs,
    budget: Budget,
    semaphore: Semaphore,
    max_tokens: AtomicU32,
    model_calls: AtomicU64,
    world_calls: AtomicU64,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl Core {
    fn reserve(counter: &AtomicU64, max: u64, what: &str) -> Result<()> {
        // A compare-exchange loop, so concurrent callers can never overshoot the limit.
        let mut current = counter.load(Ordering::SeqCst);
        loop {
            if current >= max {
                return Err(Error::BudgetExhausted(format!("{what} limit of {max} reached")));
            }
            match counter.compare_exchange(current, current + 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return Ok(()),
                Err(actual) => current = actual,
            }
        }
    }

    /// Counts a model call, or refuses it before it is made.
    pub(crate) fn reserve_model_call(&self) -> Result<()> {
        Self::reserve(&self.model_calls, self.budget.max_model_calls, "model call")
    }

    pub(crate) fn reserve_world_call(&self) -> Result<()> {
        Self::reserve(&self.world_calls, self.budget.max_world_calls, "world call")
    }

    pub(crate) async fn permit(&self) -> SemaphorePermit<'_> {
        self.semaphore
            .acquire()
            .await
            .expect("the semaphore is never closed")
    }

    pub(crate) fn max_tokens(&self) -> u32 {
        self.max_tokens.load(Ordering::SeqCst)
    }
}

/// Run one operation, with memoisation. `parent` is the calling frame, if any.
pub(crate) async fn run_op<O: Op>(
    core: &Arc<Core>,
    parent: Option<&Arc<Frame>>,
    op: &O,
    mut input: O::In,
    force: bool,
) -> Result<O::Out> {
    O::normalize(&mut input);
    let input_json = serde_json::to_value(&input)?;
    let effect = <O::Fx as Fx>::EFFECT;
    let model_id = if effect == Effect::Model {
        core.plugs.model.id()
    } else {
        ""
    };
    let key = memo_key(O::NAME, O::VERSION, model_id, &input_json);

    if !force {
        if let Some(entry) = core.memo.get(&key) {
            if entry.is_valid(&core.ledger) {
                if let Ok(out) = serde_json::from_value::<O::Out>(entry.output.clone()) {
                    core.hits.fetch_add(1, Ordering::SeqCst);
                    if let Some(p) = parent {
                        p.merge(&entry.deps);
                    }
                    return Ok(out);
                }
            } else if effect == Effect::World {
                // A World result is permanent: returned as stored, rerun only
                // with force. The caller still depends on what it was derived
                // from, so its stored deps (with their old versions) are passed
                // up. The caller then goes stale when those slots move.
                if let Ok(out) = serde_json::from_value::<O::Out>(entry.output.clone()) {
                    core.hits.fetch_add(1, Ordering::SeqCst);
                    if let Some(p) = parent {
                        p.merge(&entry.deps);
                    }
                    return Ok(out);
                }
            }
        }
    }

    core.misses.fetch_add(1, Ordering::SeqCst);
    let frame = Frame::new();
    let made_by = MadeBy::Op {
        name: O::NAME.to_string(),
        version: O::VERSION,
        effect,
    };
    let ctx = Ctx::<O::Fx>::new(core.clone(), frame.clone(), made_by);
    let result = op.run(&ctx, input).await;

    match result {
        Ok(out) => {
            let deps = frame.deps();
            core.memo.put(MemoEntry {
                key,
                op: O::NAME.to_string(),
                effect,
                input: input_json,
                output: serde_json::to_value(&out)?,
                deps: deps.clone(),
                at: now_millis(),
            })?;
            if let Some(p) = parent {
                p.absorb(&frame, Some(&deps));
            }
            Ok(out)
        }
        Err(e) => {
            // Errors are never memoised, but what was appended before the
            // failure is still in the ledger and still counts as the caller's own.
            if let Some(p) = parent {
                p.absorb(&frame, None);
            }
            Err(e)
        }
    }
}

pub struct Engine {
    core: Arc<Core>,
}

impl Engine {
    /// Open (or create) the state directory and replay it.
    pub fn open(dir: &Path, plugs: Plugs, budget: Budget) -> Result<Engine> {
        let ledger = Ledger::open(dir)?;
        let memo = MemoStore::open(dir)?;
        let permits = budget.max_concurrency.max(1);
        Ok(Engine {
            core: Arc::new(Core {
                ledger,
                memo,
                plugs,
                budget,
                semaphore: Semaphore::new(permits),
                max_tokens: AtomicU32::new(DEFAULT_MAX_TOKENS),
                model_calls: AtomicU64::new(0),
                world_calls: AtomicU64::new(0),
                hits: AtomicU64::new(0),
                misses: AtomicU64::new(0),
            }),
        })
    }

    /// Token limit passed to the model on each call.
    pub fn set_max_tokens(&self, n: u32) {
        self.core.max_tokens.store(n, Ordering::SeqCst);
    }

    pub async fn run<O: Op>(&self, op: &O, input: O::In) -> Result<O::Out> {
        run_op(&self.core, None, op, input, false).await
    }

    /// Run an operation again even if a result is stored. For World operations
    /// this is the only way to rerun one.
    pub async fn force<O: Op>(&self, op: &O, input: O::In) -> Result<O::Out> {
        run_op(&self.core, None, op, input, true).await
    }

    /// Append a thing entered by the user.
    pub fn add<T: Payload>(&self, payload: T, parents: &[ThingId]) -> Result<Id<T>> {
        Ok(self.core.ledger.put(&payload, parents, MadeBy::User)?.0)
    }

    /// Heads are set only through the engine, never from inside an operation.
    pub fn set_head(&self, name: &str, id: ThingId) -> Result<()> {
        self.core.ledger.set_head(name, id)?;
        Ok(())
    }

    /// Mark a proposal as established. The only API that takes a `Verified`.
    pub fn establish(&self, v: Verified<Id<Proposal>>) -> Result<()> {
        let proposal_id = *v.value();
        let proposal: Proposal = self.get(proposal_id)?;
        let map: Map = self.get(proposal.map)?;
        let head = format!("established/{}/{}", map.name, proposal.part);
        self.set_head(&head, proposal_id.raw())
    }

    /// Memo entries whose dependencies have moved, sorted by operation then key.
    pub fn stale(&self) -> Vec<MemoEntry> {
        let mut v: Vec<MemoEntry> = self
            .core
            .memo
            .all()
            .into_iter()
            .filter(|e| !e.is_valid(&self.core.ledger))
            .collect();
        v.sort_by(|a, b| (&a.op, a.key).cmp(&(&b.op, b.key)));
        v
    }

    pub fn memo_entries(&self) -> Vec<MemoEntry> {
        self.core.memo.all()
    }

    pub fn stats(&self) -> Stats {
        Stats {
            model_calls: self.core.model_calls.load(Ordering::SeqCst),
            world_calls: self.core.world_calls.load(Ordering::SeqCst),
            hits: self.core.hits.load(Ordering::SeqCst),
            misses: self.core.misses.load(Ordering::SeqCst),
        }
    }

    pub fn ledger(&self) -> &Ledger {
        &self.core.ledger
    }

    pub fn get<T: Payload>(&self, id: Id<T>) -> Result<T> {
        self.core.ledger.get_typed(id)
    }

    pub fn thing(&self, id: ThingId) -> Result<Thing> {
        self.core
            .ledger
            .get(&id)
            .ok_or_else(|| Error::NotFound(format!("thing {id}")))
    }

    /// Resolve a full id or a unique prefix of at least 8 hex characters.
    pub fn resolve(&self, s: &str) -> Result<ThingId> {
        self.core.ledger.resolve_prefix(s)
    }

    pub fn verifier_kinds(&self) -> Vec<crate::plug::TrialKind> {
        self.core.plugs.verifier.kinds()
    }
}
