#![allow(dead_code)]
//! Shared helpers: fakes wired into an engine, and a few tiny test-only operations.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use sandal::plug::{FakeModel, FakeSource, FakeVerifier};
use sandal::*;
use serde::{Deserialize, Serialize};

pub mod world;

pub struct Rig {
    pub dir: tempfile::TempDir,
    pub engine: Engine,
    pub model: Arc<FakeModel>,
}

pub fn plugs(model: Arc<FakeModel>, sources: Vec<Arc<FakeSource>>, verifier: Arc<FakeVerifier>) -> Plugs {
    Plugs {
        model,
        sources: sources.into_iter().map(|s| s as Arc<dyn Source>).collect(),
        verifier,
    }
}

/// An engine over a fresh temporary directory, with no sources and an empty verifier.
pub fn rig(model: FakeModel) -> Rig {
    rig_with(model, Budget::default())
}

pub fn rig_with(model: FakeModel, budget: Budget) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let model = Arc::new(model);
    let p = plugs(model.clone(), vec![], Arc::new(FakeVerifier::new(vec![])));
    let engine = Engine::open(dir.path(), p, budget).unwrap();
    Rig { dir, engine, model }
}

pub fn idea(n: &str) -> Idea {
    Idea { title: n.into(), body: format!("body of {n}") }
}

// ---- test-only operations -------------------------------------------------

/// Model op: asks the model once and returns its reply.
pub struct AskOp;

#[derive(Clone, Serialize, Deserialize)]
pub struct AskIn {
    pub text: String,
}

#[async_trait]
impl Op for AskOp {
    type Fx = ModelFx;
    type In = AskIn;
    type Out = String;
    const NAME: &'static str = "ask_op";
    const VERSION: u32 = 1;
    async fn run(&self, ctx: &Ctx<ModelFx>, i: AskIn) -> Result<String> {
        ctx.ask("system", &i.text).await
    }
}

/// Pure op: reads children(x), then appends a new child of x. Returns how many
/// children it saw.
pub struct ReadAndAppend {
    pub runs: AtomicUsize,
}

impl ReadAndAppend {
    pub fn new() -> Self {
        ReadAndAppend { runs: AtomicUsize::new(0) }
    }
    pub fn runs(&self) -> usize {
        self.runs.load(Ordering::SeqCst)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct XIn {
    pub x: ThingId,
}

#[async_trait]
impl Op for ReadAndAppend {
    type Fx = PureFx;
    type In = XIn;
    type Out = usize;
    const NAME: &'static str = "read_and_append";
    const VERSION: u32 = 1;
    async fn run(&self, ctx: &Ctx<PureFx>, i: XIn) -> Result<usize> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let seen = ctx.children(i.x).len();
        ctx.put(idea("appended by op"), &[i.x])?;
        Ok(seen)
    }
}

/// Pure op that appends a child of x with the given tag (a "different operation").
pub struct Append;

#[derive(Clone, Serialize, Deserialize)]
pub struct AppendIn {
    pub x: ThingId,
    pub tag: String,
}

#[async_trait]
impl Op for Append {
    type Fx = PureFx;
    type In = AppendIn;
    type Out = ThingId;
    const NAME: &'static str = "append";
    const VERSION: u32 = 1;
    async fn run(&self, ctx: &Ctx<PureFx>, i: AppendIn) -> Result<ThingId> {
        Ok(ctx.put(idea(&i.tag), &[i.x])?.raw())
    }
}

/// Pure op that fails the first `fail_times` runs, then succeeds.
pub struct Flaky {
    pub runs: AtomicUsize,
    pub fail_times: usize,
}

#[async_trait]
impl Op for Flaky {
    type Fx = PureFx;
    type In = AskIn;
    type Out = String;
    const NAME: &'static str = "flaky";
    const VERSION: u32 = 1;
    async fn run(&self, _ctx: &Ctx<PureFx>, i: AskIn) -> Result<String> {
        let n = self.runs.fetch_add(1, Ordering::SeqCst);
        if n < self.fail_times {
            Err(Error::Plug("flaky failure".into()))
        } else {
            Ok(i.text)
        }
    }
}

/// Two versions of one op, differing only in `VERSION`.
pub struct VersionOne;
pub struct VersionTwo;

#[async_trait]
impl Op for VersionOne {
    type Fx = ModelFx;
    type In = AskIn;
    type Out = String;
    const NAME: &'static str = "versioned";
    const VERSION: u32 = 1;
    async fn run(&self, ctx: &Ctx<ModelFx>, i: AskIn) -> Result<String> {
        ctx.ask("system", &i.text).await
    }
}

#[async_trait]
impl Op for VersionTwo {
    type Fx = ModelFx;
    type In = AskIn;
    type Out = String;
    const NAME: &'static str = "versioned";
    const VERSION: u32 = 2;
    async fn run(&self, ctx: &Ctx<ModelFx>, i: AskIn) -> Result<String> {
        ctx.ask("system", &i.text).await
    }
}

/// World op that reads children(x) and counts how often it really ran.
pub struct WorldCount {
    pub runs: AtomicUsize,
}

#[async_trait]
impl Op for WorldCount {
    type Fx = WorldFx;
    type In = XIn;
    type Out = usize;
    const NAME: &'static str = "world_count";
    const VERSION: u32 = 1;
    async fn run(&self, ctx: &Ctx<WorldFx>, i: XIn) -> Result<usize> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok(ctx.children(i.x).len())
    }
}
