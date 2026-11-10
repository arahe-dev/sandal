mod common;
use async_trait::async_trait;
use common::*;
use sandal::plug::FakeModel;
use sandal::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[tokio::test]
async fn world_is_never_auto_rerun() {
    let r = rig(FakeModel::scripted(&[]));
    let x = r.engine.add(idea("x"), &[]).unwrap().raw();
    let op = WorldCount { runs: AtomicUsize::new(0) };
    assert_eq!(r.engine.run(&op, XIn { x }).await.unwrap(), 0);
    // Its dependency changes...
    r.engine.run(&Append, AppendIn { x, tag: "new".into() }).await.unwrap();
    assert!(r.engine.stale().iter().any(|e| e.op == "world_count"));
    // ...and it is still returned as stored.
    assert_eq!(r.engine.run(&op, XIn { x }).await.unwrap(), 0);
    assert_eq!(op.runs.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn force_reruns_world() {
    let r = rig(FakeModel::scripted(&[]));
    let x = r.engine.add(idea("x"), &[]).unwrap().raw();
    let op = WorldCount { runs: AtomicUsize::new(0) };
    r.engine.run(&op, XIn { x }).await.unwrap();
    r.engine.run(&Append, AppendIn { x, tag: "new".into() }).await.unwrap();
    assert_eq!(r.engine.force(&op, XIn { x }).await.unwrap(), 1);
    assert_eq!(op.runs.load(Ordering::SeqCst), 2);
}

struct Reader;

#[async_trait]
impl Op for Reader {
    type Fx = ModelFx;
    type In = XIn;
    type Out = String;
    const NAME: &'static str = "reader";
    const VERSION: u32 = 1;
    async fn run(&self, ctx: &Ctx<ModelFx>, i: XIn) -> Result<String> {
        let n = ctx.children(i.x).len();
        ctx.ask("system", &format!("children: {n}")).await
    }
}

#[tokio::test]
async fn model_ops_are_rerun_when_stale() {
    // The contrast with World: a stale Model result is recomputed and counts against the budget.
    let r = rig(FakeModel::scripted(&[("children", "ok")]));
    let x = r.engine.add(idea("x"), &[]).unwrap().raw();
    r.engine.run(&Reader, XIn { x }).await.unwrap();
    r.engine.run(&Reader, XIn { x }).await.unwrap();
    assert_eq!(r.model.calls(), 1);
    r.engine.run(&Append, AppendIn { x, tag: "new".into() }).await.unwrap();
    r.engine.run(&Reader, XIn { x }).await.unwrap();
    assert_eq!(r.model.calls(), 2);
}

#[tokio::test]
async fn budget_stops_calls() {
    let r = rig_with(
        FakeModel::scripted(&[("q", "a")]),
        Budget { max_model_calls: 2, ..Budget::default() },
    );
    let q = |n: u8| AskIn { text: format!("q{n}") };
    r.engine.run(&AskOp, q(1)).await.unwrap();
    r.engine.run(&AskOp, q(2)).await.unwrap();
    // A hit costs nothing, even at the limit.
    r.engine.run(&AskOp, q(1)).await.unwrap();
    let err = r.engine.run(&AskOp, q(3)).await.unwrap_err();
    assert!(matches!(err, Error::BudgetExhausted(_)), "{err:?}");
    assert_eq!(r.model.calls(), 2);
    assert_eq!(r.engine.stats().model_calls, 2);
}

struct Fan;

#[async_trait]
impl Op for Fan {
    type Fx = ModelFx;
    type In = ();
    type Out = Vec<String>;
    const NAME: &'static str = "fan";
    const VERSION: u32 = 1;
    async fn run(&self, ctx: &Ctx<ModelFx>, _: ()) -> Result<Vec<String>> {
        let inputs = (0..8).map(|n| AskIn { text: format!("q{n}") }).collect();
        ctx.run_all(&AskOp, inputs).await.into_iter().collect()
    }
}

#[tokio::test]
async fn run_all_is_bounded_and_parallel() {
    let r = rig_with(
        FakeModel::scripted(&[("q", "a")]).with_delay(Duration::from_millis(50)),
        Budget { max_concurrency: 4, ..Budget::default() },
    );
    let start = Instant::now();
    let out = r.engine.run(&Fan, ()).await.unwrap();
    let took = start.elapsed();
    assert_eq!(out.len(), 8);
    assert!(took < Duration::from_millis(300), "took {took:?}");
    assert!(r.model.peak_in_flight() <= 4, "peak {}", r.model.peak_in_flight());
    assert!(r.model.peak_in_flight() >= 2, "ran in parallel");
}

struct Ordered;

#[async_trait]
impl Op for Ordered {
    type Fx = ModelFx;
    type In = ();
    type Out = Vec<String>;
    const NAME: &'static str = "ordered";
    const VERSION: u32 = 1;
    async fn run(&self, ctx: &Ctx<ModelFx>, _: ()) -> Result<Vec<String>> {
        let inputs = ["three", "one", "two"].iter().map(|t| AskIn { text: t.to_string() }).collect();
        ctx.run_all(&AskOp, inputs).await.into_iter().collect()
    }
}

#[tokio::test]
async fn run_all_keeps_input_order() {
    let r = rig(FakeModel::scripted(&[("one", "1"), ("two", "2"), ("three", "3")]));
    assert_eq!(r.engine.run(&Ordered, ()).await.unwrap(), vec!["3", "1", "2"]);
}

mod with_world {
    use super::common::world::*;
    use sandal::plug::FakeVerifier;
    use sandal::*;

    async fn first_proposal(w: &World) -> Id<Proposal> {
        let map = make_map(w).await;
        let out = w
            .engine
            .run(&Cycle, CycleIn { map, k: 10, max_trials: 5, exclude_domains: vec![] })
            .await
            .unwrap();
        w.engine.get(out.study).unwrap().order[0]
    }

    #[tokio::test]
    async fn try_is_never_auto_rerun() {
        let w = world();
        let proposal = first_proposal(&w).await;
        let first = w.engine.run(&Try, TryIn { proposal }).await.unwrap();
        // New children appear under the proposal, which is what Try's inputs sit on.
        w.engine
            .add(Idea { title: "x".into(), body: "y".into() }, &[proposal.raw()])
            .unwrap();
        let second = w.engine.run(&Try, TryIn { proposal }).await.unwrap();
        assert_eq!(first, second);
        assert_eq!(w.verifier.calls(), 1);
    }

    #[tokio::test]
    async fn force_reruns_try() {
        let w = world();
        let proposal = first_proposal(&w).await;
        w.engine.run(&Try, TryIn { proposal }).await.unwrap();
        w.engine.force(&Try, TryIn { proposal }).await.unwrap();
        assert_eq!(w.verifier.calls(), 2);
    }

    #[tokio::test]
    async fn world_budget_stops_calls() {
        let w = world();
        let proposal = first_proposal(&w).await;
        // Same engine rules, tighter world budget.
        let model = std::sync::Arc::new(sandal::plug::FakeModel::new(script()));
        let plugs = Plugs {
            model,
            sources: vec![],
            verifier: std::sync::Arc::new(FakeVerifier::new(vec![("check", Ok(FakeVerifier::outcome(Grade::Confirmed, "ok")))])),
        };
        let dir = w.dir.path().to_path_buf();
        let World { engine, .. } = w;
        drop(engine);
        let engine = Engine::open(&dir, plugs, Budget { max_world_calls: 0, ..Budget::default() }).unwrap();
        let err = engine.run(&Try, TryIn { proposal }).await.unwrap_err();
        assert!(matches!(err, Error::BudgetExhausted(_)), "{err:?}");
    }
}
