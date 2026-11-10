//! The context an operation runs in. An operation touches the ledger, the
//! model and other operations only through its `Ctx`, which is how
//! dependencies get recorded.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use serde::de::DeserializeOwned;

use crate::engine::{run_op, Core};
use crate::error::{Error, Result};
use crate::fx::{Allows, Fx, ModelFx, WorldFx};
use crate::id::{Id, ThingId};
use crate::ledger::Slot;
use crate::op::Op;
use crate::plug::{SourceQuery, TrialKind};
use crate::thing::{Grade, MadeBy, Payload, Proposal, SourceItem, Thing, TrialResult};

/// Dependencies and own appends of one running operation.
#[derive(Default)]
pub(crate) struct Frame {
    state: Mutex<FrameState>,
}

#[derive(Default)]
struct FrameState {
    /// Every slot read, with the version to record for it.
    deps: BTreeMap<Slot, u64>,
    /// How many times this frame (or an operation it called) bumped each slot.
    bumps: HashMap<Slot, u64>,
}

impl Frame {
    pub(crate) fn new() -> Arc<Frame> {
        Arc::new(Frame::default())
    }

    /// Record a read. The first version seen is kept.
    fn read(&self, slot: Slot, version: u64) {
        self.state.lock().unwrap().deps.entry(slot).or_insert(version);
    }

    /// The own-append rule: a bump this frame caused itself must not make its
    /// own entry stale, so a slot already read has its recorded version raised.
    fn bump(&self, slot: Slot, n: u64) {
        let mut st = self.state.lock().unwrap();
        *st.bumps.entry(slot.clone()).or_insert(0) += n;
        if let Some(v) = st.deps.get_mut(&slot) {
            *v += n;
        }
    }

    pub(crate) fn deps(&self) -> Vec<(Slot, u64)> {
        self.state
            .lock()
            .unwrap()
            .deps
            .iter()
            .map(|(s, v)| (s.clone(), *v))
            .collect()
    }

    /// Fold a finished child frame into this one: first its bumps (they count
    /// as this frame's own), then, when the child succeeded, its dependencies.
    pub(crate) fn absorb(&self, child: &Frame, deps: Option<&[(Slot, u64)]>) {
        let bumps: Vec<(Slot, u64)> = child
            .state
            .lock()
            .unwrap()
            .bumps
            .iter()
            .map(|(s, n)| (s.clone(), *n))
            .collect();
        for (s, n) in bumps {
            self.bump(s, n);
        }
        if let Some(deps) = deps {
            self.merge(deps);
        }
    }

    /// Add dependencies from a memo hit or a finished child.
    pub(crate) fn merge(&self, deps: &[(Slot, u64)]) {
        let mut st = self.state.lock().unwrap();
        for (s, v) in deps {
            st.deps.entry(s.clone()).or_insert(*v);
        }
    }
}

/// The handle an operation gets. `E` is its effect marker.
pub struct Ctx<E: Fx> {
    pub(crate) core: Arc<Core>,
    pub(crate) frame: Arc<Frame>,
    pub(crate) made_by: MadeBy,
    _e: PhantomData<fn() -> E>,
}

impl<E: Fx> Ctx<E> {
    pub(crate) fn new(core: Arc<Core>, frame: Arc<Frame>, made_by: MadeBy) -> Self {
        Ctx {
            core,
            frame,
            made_by,
            _e: PhantomData,
        }
    }

    /// Load a typed payload. Things never change, so this records no dependency.
    pub fn get<T: Payload>(&self, id: Id<T>) -> Result<T> {
        self.core.ledger.get_typed(id)
    }

    pub fn thing(&self, id: ThingId) -> Result<Thing> {
        self.core
            .ledger
            .get(&id)
            .ok_or_else(|| Error::NotFound(format!("thing {id}")))
    }

    /// Children of a thing, sorted by id. Records `Slot::Children(id)`.
    pub fn children(&self, id: ThingId) -> Vec<ThingId> {
        let (kids, version) = self.core.ledger.children_with_version(&id);
        self.frame.read(Slot::Children(id), version);
        kids
    }

    /// Where a head points. Records `Slot::Head(name)`.
    pub fn head(&self, name: &str) -> Option<ThingId> {
        let (id, version) = self.core.ledger.head_with_version(name);
        self.frame.read(Slot::Head(name.to_string()), version);
        id
    }

    /// Append a thing made by the running operation.
    pub fn put<T: Payload>(&self, payload: T, parents: &[ThingId]) -> Result<Id<T>> {
        self.put_as(self.made_by.clone(), payload, parents)
    }

    fn put_as<T: Payload>(&self, made_by: MadeBy, payload: T, parents: &[ThingId]) -> Result<Id<T>> {
        let (id, appended) = self.core.ledger.put(&payload, parents, made_by)?;
        for slot in appended.bumped {
            self.frame.bump(slot, 1);
        }
        Ok(id)
    }

    /// The trial kinds the verifier accepts.
    pub fn trial_kinds(&self) -> Vec<TrialKind> {
        self.core.plugs.verifier.kinds()
    }

    /// Run another operation. Its dependencies flow into this frame.
    pub async fn run<O: Op>(&self, op: &O, input: O::In) -> Result<O::Out>
    where
        E: Allows<O::Fx>,
    {
        run_op(&self.core, Some(&self.frame), op, input, false).await
    }

    /// Runs concurrently. Order of results = order of inputs.
    ///
    /// DECISION: concurrency is bounded where the outside world is touched (one
    /// shared semaphore around every model, source and verifier call, sized by
    /// `Budget::max_concurrency`), not around whole operations. A permit is
    /// never held while waiting on another operation, so nested `run_all`
    /// calls cannot deadlock.
    pub async fn run_all<O: Op>(&self, op: &O, inputs: Vec<O::In>) -> Vec<Result<O::Out>>
    where
        E: Allows<O::Fx>,
    {
        futures::future::join_all(inputs.into_iter().map(|i| self.run(op, i))).await
    }
}

impl<E: Allows<ModelFx>> Ctx<E> {
    pub async fn ask(&self, system: &str, prompt: &str) -> Result<String> {
        self.core.reserve_model_call()?;
        let _permit = self.core.permit().await;
        let max_tokens = self.core.max_tokens();
        self.core.plugs.model.complete(system, prompt, max_tokens).await
    }

    /// Strips code fences, parses JSON. On a parse failure, retries once with the
    /// parse error appended to the prompt. A second failure is `Error::BadModelOutput`.
    pub async fn ask_json<T: DeserializeOwned>(&self, system: &str, prompt: &str) -> Result<T> {
        let first = self.ask(system, prompt).await?;
        match parse_json::<T>(&first) {
            Ok(v) => Ok(v),
            Err(e) => {
                let retry = retry_prompt(prompt, &e);
                let second = self.ask(system, &retry).await?;
                parse_json::<T>(&second).map_err(|e2| {
                    Error::BadModelOutput(format!("could not parse model reply as JSON: {e2}"))
                })
            }
        }
    }
}

/// Remove a surrounding markdown code fence, if there is one.
pub fn strip_fences(s: &str) -> &str {
    let s = s.trim();
    let Some(rest) = s.strip_prefix("```") else {
        return s;
    };
    let rest = match rest.find('\n') {
        Some(i) => &rest[i + 1..],
        None => rest,
    };
    rest.trim_end().strip_suffix("```").unwrap_or(rest).trim()
}

pub(crate) fn parse_json<T: DeserializeOwned>(raw: &str) -> std::result::Result<T, String> {
    serde_json::from_str(strip_fences(raw)).map_err(|e| e.to_string())
}

pub(crate) fn retry_prompt(prompt: &str, error: &str) -> String {
    format!(
        "{prompt}\n\nYour previous reply could not be parsed: {error}\nReturn only the corrected JSON object."
    )
}

impl Ctx<WorldFx> {
    /// Ask every source in order, append each item, drop duplicates by id and
    /// return the first `k`.
    pub async fn fetch(&self, q: &SourceQuery) -> Result<Vec<Id<SourceItem>>> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for source in &self.core.plugs.sources {
            self.core.reserve_world_call()?;
            let items = {
                let _permit = self.core.permit().await;
                source.fetch(q).await?
            };
            for item in items {
                let id = self.put_as(MadeBy::Source(source.name().to_string()), item, &[])?;
                if seen.insert(id) {
                    out.push(id);
                }
            }
        }
        out.truncate(q.k);
        Ok(out)
    }

    /// Run a proposal's trial. A verifier error or timeout still appends a
    /// result, graded Uncertain, with the error text in `observed`.
    pub async fn verify(&self, proposal: Id<Proposal>) -> Result<Id<TrialResult>> {
        let p = self.get(proposal)?;
        self.core.reserve_world_call()?;
        let verifier = &self.core.plugs.verifier;
        let outcome = {
            let _permit = self.core.permit().await;
            verifier.run(&p.trial).await
        };
        let (grade, observed, metrics) = match outcome {
            Ok(o) => (o.grade, o.observed, o.metrics),
            Err(e) => (Grade::Uncertain, e.to_string(), BTreeMap::new()),
        };
        self.put_as(
            MadeBy::Verifier(verifier.name().to_string()),
            TrialResult {
                proposal,
                grade,
                observed,
                metrics,
            },
            &[proposal.raw()],
        )
    }
}
