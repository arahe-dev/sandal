//! Fakes for the three plug-in traits. Every test runs against these.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;

use super::{Model, Source, SourceQuery, TrialKind, TrialOutcome, Verifier};
use crate::error::{Error, Result};
use crate::thing::{Grade, SourceItem, TrialSpec};

/// Built from a list of (needle, response) pairs. Returns the response of the
/// first pair whose needle occurs in the prompt, else `Error::Plug`. Records
/// every prompt it receives, counts calls, and takes an optional per-call delay.
pub struct FakeModel {
    id: String,
    script: Vec<(String, String)>,
    delay: Duration,
    prompts: Mutex<Vec<String>>,
    calls: AtomicUsize,
    in_flight: AtomicUsize,
    peak: AtomicUsize,
}

impl FakeModel {
    pub fn new(script: Vec<(String, String)>) -> Self {
        FakeModel {
            id: "fake".to_string(),
            script,
            delay: Duration::ZERO,
            prompts: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
            in_flight: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        }
    }

    /// Convenience: `FakeModel::scripted(&[("needle", "response")])`.
    pub fn scripted(pairs: &[(&str, &str)]) -> Self {
        Self::new(
            pairs
                .iter()
                .map(|(n, r)| (n.to_string(), r.to_string()))
                .collect(),
        )
    }

    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub fn with_id(mut self, id: &str) -> Self {
        self.id = id.to_string();
        self
    }

    /// Every prompt received, in order.
    pub fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// The most calls that were ever in flight at once.
    pub fn peak_in_flight(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Model for FakeModel {
    fn id(&self) -> &str {
        &self.id
    }

    async fn complete(&self, _system: &str, prompt: &str, _max_tokens: u32) -> Result<String> {
        self.prompts.lock().unwrap().push(prompt.to_string());
        self.calls.fetch_add(1, Ordering::SeqCst);
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.script
            .iter()
            .find(|(needle, _)| prompt.contains(needle.as_str()))
            .map(|(_, response)| response.clone())
            .ok_or_else(|| Error::Plug("FakeModel: no scripted response matches the prompt".into()))
    }
}

/// Returns a fixed list. Counts calls.
pub struct FakeSource {
    name: String,
    items: Vec<SourceItem>,
    calls: AtomicUsize,
}

impl FakeSource {
    pub fn new(name: &str, items: Vec<SourceItem>) -> Self {
        FakeSource {
            name: name.to_string(),
            items,
            calls: AtomicUsize::new(0),
        }
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Source for FakeSource {
    fn name(&self) -> &str {
        &self.name
    }

    async fn fetch(&self, _q: &SourceQuery) -> Result<Vec<SourceItem>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.items.clone())
    }
}

/// Built from a map of trial kind to either an outcome or an error. Counts calls.
pub struct FakeVerifier {
    outcomes: BTreeMap<String, std::result::Result<TrialOutcome, String>>,
    calls: AtomicUsize,
}

impl FakeVerifier {
    pub fn new(entries: Vec<(&str, std::result::Result<TrialOutcome, String>)>) -> Self {
        FakeVerifier {
            outcomes: entries
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            calls: AtomicUsize::new(0),
        }
    }

    /// An outcome with the given grade and no metrics.
    pub fn outcome(grade: Grade, observed: &str) -> TrialOutcome {
        TrialOutcome {
            grade,
            observed: observed.to_string(),
            metrics: BTreeMap::new(),
        }
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Verifier for FakeVerifier {
    fn name(&self) -> &str {
        "fake"
    }

    fn kinds(&self) -> Vec<TrialKind> {
        self.outcomes
            .keys()
            .map(|k| TrialKind {
                name: k.clone(),
                params: Vec::new(),
                description: format!("fake trial {k}"),
            })
            .collect()
    }

    async fn run(&self, spec: &TrialSpec) -> Result<TrialOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.outcomes.get(&spec.kind) {
            Some(Ok(o)) => Ok(o.clone()),
            Some(Err(e)) => Err(Error::Plug(e.clone())),
            None => Err(Error::Plug(format!("FakeVerifier: unknown kind {}", spec.kind))),
        }
    }
}
