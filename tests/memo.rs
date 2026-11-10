mod common;
use common::*;
use sandal::plug::{FakeModel, FakeVerifier};
use sandal::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[tokio::test]
async fn second_run_is_a_hit() {
    let r = rig(FakeModel::scripted(&[("hello", "world")]));
    let input = || AskIn { text: "hello".into() };
    assert_eq!(r.engine.run(&AskOp, input()).await.unwrap(), "world");
    assert_eq!(r.engine.run(&AskOp, input()).await.unwrap(), "world");
    assert_eq!(r.model.calls(), 1);
    let s = r.engine.stats();
    assert_eq!((s.hits, s.misses, s.model_calls), (1, 1, 1));
}

#[tokio::test]
async fn own_append_does_not_self_invalidate() {
    let r = rig(FakeModel::scripted(&[]));
    let x = r.engine.add(idea("x"), &[]).unwrap().raw();
    let op = ReadAndAppend::new();
    assert_eq!(r.engine.run(&op, XIn { x }).await.unwrap(), 0);
    // It appended a child of x itself, and must still be a hit.
    assert_eq!(r.engine.run(&op, XIn { x }).await.unwrap(), 0);
    assert_eq!(op.runs(), 1);
}

#[tokio::test]
async fn other_append_does_invalidate() {
    let r = rig(FakeModel::scripted(&[]));
    let x = r.engine.add(idea("x"), &[]).unwrap().raw();
    let op = ReadAndAppend::new();
    r.engine.run(&op, XIn { x }).await.unwrap();
    r.engine.run(&Append, AppendIn { x, tag: "other".into() }).await.unwrap();
    // Two children now: the op's own and the other operation's.
    assert_eq!(r.engine.run(&op, XIn { x }).await.unwrap(), 2);
    assert_eq!(op.runs(), 2);
}

#[tokio::test]
async fn errors_are_not_memoised() {
    let r = rig(FakeModel::scripted(&[]));
    let op = Flaky { runs: AtomicUsize::new(0), fail_times: 1 };
    let input = || AskIn { text: "t".into() };
    assert!(r.engine.run(&op, input()).await.is_err());
    assert_eq!(r.engine.run(&op, input()).await.unwrap(), "t");
    assert_eq!(op.runs.load(Ordering::SeqCst), 2);
    // And the success is memoised.
    r.engine.run(&op, input()).await.unwrap();
    assert_eq!(op.runs.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn op_version_bump_misses() {
    let r = rig(FakeModel::scripted(&[("q", "a")]));
    let input = || AskIn { text: "q".into() };
    r.engine.run(&VersionOne, input()).await.unwrap();
    r.engine.run(&VersionOne, input()).await.unwrap();
    assert_eq!(r.model.calls(), 1);
    r.engine.run(&VersionTwo, input()).await.unwrap();
    assert_eq!(r.model.calls(), 2);
}

fn reopen(dir: &std::path::Path, model_id: &str) -> (Engine, Arc<FakeModel>) {
    let model = Arc::new(FakeModel::scripted(&[("q", "a")]).with_id(model_id));
    let p = plugs(model.clone(), vec![], Arc::new(FakeVerifier::new(vec![])));
    (Engine::open(dir, p, Budget::default()).unwrap(), model)
}

#[tokio::test]
async fn model_id_is_part_of_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let input = || AskIn { text: "q".into() };
    {
        let (e, m) = reopen(dir.path(), "model-a");
        e.run(&AskOp, input()).await.unwrap();
        assert_eq!(m.calls(), 1);
    }
    let (e, m) = reopen(dir.path(), "model-a");
    e.run(&AskOp, input()).await.unwrap();
    assert_eq!(m.calls(), 0, "same model id: a hit after reopening");
    let (e, m) = reopen(dir.path(), "model-b");
    e.run(&AskOp, input()).await.unwrap();
    assert_eq!(m.calls(), 1, "a different model id misses");
}

#[tokio::test]
async fn memo_survives_restart_at_the_op_level() {
    // Memo validity survives a restart because slot versions are derived from the ledger.
    let dir = tempfile::tempdir().unwrap();
    let (e, _) = reopen(dir.path(), "m");
    let x = e.add(idea("x"), &[]).unwrap().raw();
    let op = ReadAndAppend::new();
    e.run(&op, XIn { x }).await.unwrap();
    drop(e);
    let (e, _) = reopen(dir.path(), "m");
    let op2 = ReadAndAppend::new();
    e.run(&op2, XIn { x }).await.unwrap();
    assert_eq!(op2.runs(), 0);
    assert!(e.stale().is_empty());
}

mod with_world {
    use super::common::world::{self, *};
    use sandal::plug::{FakeModel, FakeSource, FakeVerifier};
    use sandal::*;
    use std::sync::Arc;

    fn cycle_in(map: Id<Map>) -> CycleIn {
        CycleIn { map, k: 10, max_trials: 5, exclude_domains: vec![] }
    }

    #[tokio::test]
    async fn persisted_across_restart() {
        // Cycle, reopen the directory, Cycle again: zero model calls on the second.
        let w = world();
        let map = make_map(&w).await;
        let first = w.engine.run(&Cycle, cycle_in(map)).await.unwrap();
        assert!(w.model.calls() > 0);
        let World { dir, engine, .. } = w;
        drop(engine);

        let model = Arc::new(FakeModel::new(world::script()));
        let plugs = Plugs {
            model: model.clone(),
            sources: vec![Arc::new(FakeSource::new("fake", world::items()))],
            verifier: Arc::new(FakeVerifier::new(vec![("check", Ok(FakeVerifier::outcome(Grade::Confirmed, "ok")))])),
        };
        let engine = Engine::open(dir.path(), plugs, Budget::default()).unwrap();
        let again = engine.run(&Cycle, cycle_in(map)).await.unwrap();
        assert_eq!(again.study, first.study);
        assert_eq!(model.calls(), 0);
        assert_eq!(engine.stats().misses, 0);
    }

    #[tokio::test]
    async fn new_evidence_invalidates_only_dependents() {
        let w = world();
        let map = make_map(&w).await;
        let out = w.engine.run(&Cycle, cycle_in(map)).await.unwrap();
        let study = w.engine.get(out.study).unwrap();

        // Source A's proposal is the odour one; B's is the waste one.
        let prop_of = |part: &str| {
            *study.order.iter().find(|p| w.engine.get(**p).unwrap().part == part).unwrap()
        };
        let (pa, pb) = (prop_of("odour"), prop_of("waste"));
        let a = w.engine.get(pa).unwrap().source;
        let b = w.engine.get(pb).unwrap().source;
        assert_ne!(a, b);

        w.engine.run(&Read, ReadIn { thing: a, map }).await.unwrap();
        w.engine.run(&Read, ReadIn { thing: b, map }).await.unwrap();
        let reads = |w: &World| {
            w.engine.stale().into_iter().filter(|e| e.op == "read").collect::<Vec<_>>()
        };
        assert!(reads(&w).is_empty(), "fresh readings are not stale");

        // New evidence under a proposal carried from A.
        w.engine.run(&Try, TryIn { proposal: pa }).await.unwrap();

        let stale = reads(&w);
        assert_eq!(stale.len(), 1);
        let thing: ThingId = serde_json::from_value(stale[0].input["thing"].clone()).unwrap();
        assert_eq!(thing, a);

        // Running both again makes exactly one more model call.
        let before = w.model.calls();
        w.engine.run(&Read, ReadIn { thing: a, map }).await.unwrap();
        w.engine.run(&Read, ReadIn { thing: b, map }).await.unwrap();
        assert_eq!(w.model.calls(), before + 1);
    }
}
