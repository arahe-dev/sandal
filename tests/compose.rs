mod common;
use async_trait::async_trait;
use common::world::{self, *};
use common::*;
use sandal::plug::FakeModel;
use sandal::*;

// The example includes tests/common/world.rs too, which `mod common` already loads.
#[path = "../examples/try_and_read.rs"]
#[allow(dead_code, clippy::duplicate_mod)]
mod try_and_read;

struct Inc;
struct Double;

#[async_trait]
impl Op for Inc {
    type Fx = PureFx;
    type In = u32;
    type Out = u32;
    const NAME: &'static str = "inc";
    const VERSION: u32 = 1;
    async fn run(&self, _: &Ctx<PureFx>, i: u32) -> Result<u32> {
        Ok(i + 1)
    }
}

#[async_trait]
impl Op for Double {
    type Fx = PureFx;
    type In = u32;
    type Out = u32;
    const NAME: &'static str = "double";
    const VERSION: u32 = 1;
    async fn run(&self, _: &Ctx<PureFx>, i: u32) -> Result<u32> {
        Ok(i * 2)
    }
}

struct IncThenDouble;

#[async_trait]
impl Op for IncThenDouble {
    type Fx = PureFx;
    type In = u32;
    type Out = u32;
    const NAME: &'static str = "inc_then_double";
    const VERSION: u32 = 1;
    async fn run(&self, ctx: &Ctx<PureFx>, i: u32) -> Result<u32> {
        then(ctx, &Inc, &Double, i).await
    }
}

struct Climb {
    max_rounds: usize,
}

#[async_trait]
impl Op for Climb {
    type Fx = PureFx;
    type In = u32;
    type Out = Vec<u32>;
    const NAME: &'static str = "climb";
    const VERSION: u32 = 1;
    async fn run(&self, ctx: &Ctx<PureFx>, i: u32) -> Result<Vec<u32>> {
        iterate(ctx, &Inc, i, |out| (*out < 4).then_some(*out), self.max_rounds).await
    }
}

#[tokio::test]
async fn then_chains() {
    let r = rig(FakeModel::scripted(&[]));
    let by_hand = {
        let a = r.engine.run(&Inc, 3).await.unwrap();
        r.engine.run(&Double, a).await.unwrap()
    };
    assert_eq!(r.engine.run(&IncThenDouble, 3).await.unwrap(), by_hand);
    assert_eq!(by_hand, 8);
}

#[tokio::test]
async fn iterate_stops_on_none() {
    let r = rig(FakeModel::scripted(&[]));
    // 1 -> 2 -> 3 -> 4, then `next` returns None.
    assert_eq!(r.engine.run(&Climb { max_rounds: 10 }, 1).await.unwrap(), vec![2, 3, 4]);
    // Otherwise it stops at max_rounds.
    assert_eq!(r.engine.run(&Climb { max_rounds: 2 }, 0).await.unwrap().len(), 2);
}

#[tokio::test]
async fn each_runs_every_input() {
    struct Many;
    #[async_trait]
    impl Op for Many {
        type Fx = PureFx;
        type In = ();
        type Out = Vec<u32>;
        const NAME: &'static str = "many";
        const VERSION: u32 = 1;
        async fn run(&self, ctx: &Ctx<PureFx>, _: ()) -> Result<Vec<u32>> {
            each(ctx, &Inc, vec![1, 2, 3]).await.into_iter().collect()
        }
    }
    let r = rig(FakeModel::scripted(&[]));
    assert_eq!(r.engine.run(&Many, ()).await.unwrap(), vec![2, 3, 4]);
}

#[tokio::test]
async fn cycle_end_to_end() {
    let w = world();
    let map = make_map(&w).await;
    let out = w
        .engine
        .run(&Cycle, CycleIn { map, k: 10, max_trials: 5, exclude_domains: vec![] })
        .await
        .unwrap();
    let study = w.engine.get(out.study).unwrap();
    assert!(!study.order.is_empty());
    assert_eq!(study.order.len(), 2, "sources A and B each carried one proposal");
    // Source D's reply is not JSON: counted, not fatal.
    assert_eq!(out.skipped, 1);
    // Cycle runs no trials.
    assert_eq!(w.verifier.calls(), 0);
    // The study's first proposal is the one whose match lacked nothing.
    let first = w.engine.get(study.order[0]).unwrap();
    assert_eq!(first.part, "waste");
}

#[tokio::test]
async fn cycle_respects_exclude_domains_through_the_query() {
    // FakeSource ignores the query, so the exclusion is checked in plugs.rs
    // against FileSource; here only that Notice passes it through unharmed.
    let w = world();
    let map = make_map(&w).await;
    let out = w
        .engine
        .run(&Cycle, CycleIn { map, k: 10, max_trials: 5, exclude_domains: vec!["military".into()] })
        .await
        .unwrap();
    assert!(!w.engine.get(out.study).unwrap().order.is_empty());
}

#[tokio::test]
async fn try_and_read_example() {
    let w = world();
    let map = make_map(&w).await;
    let cycle = w
        .engine
        .run(&Cycle, CycleIn { map, k: 10, max_trials: 5, exclude_domains: vec![] })
        .await
        .unwrap();
    let readings = w
        .engine
        .run(&try_and_read::TryAndRead, try_and_read::TryAndReadIn { study: cycle.study, map, n: 2 })
        .await
        .unwrap();
    // One reading per tried proposal.
    assert_eq!(readings.len(), 2);
    assert_eq!(w.verifier.calls(), 2);
    for r in readings {
        // The trials were confirmed, so the readings were made with evidence in hand.
        assert_eq!(w.engine.get(r).unwrap().text, "EVIDENCE reading");
    }
    let _ = world::PROBLEM;
}
