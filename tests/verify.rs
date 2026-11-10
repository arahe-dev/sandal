mod common;
use common::world::{self, *};
use sandal::plug::FakeVerifier;
use sandal::*;

/// Run the cycle and try its first proposal; return (map, proposal, result).
async fn tried(w: &World) -> (Id<Map>, Id<Proposal>, Id<TrialResult>) {
    let map = make_map(w).await;
    let out = w
        .engine
        .run(&Cycle, CycleIn { map, k: 10, max_trials: 5, exclude_domains: vec![] })
        .await
        .unwrap();
    let proposal = w.engine.get(out.study).unwrap().order[0];
    let result = w.engine.run(&Try, TryIn { proposal }).await.unwrap();
    (map, proposal, result)
}

#[tokio::test]
async fn confirm_accepts_confirmed() {
    let w = world();
    let (_, proposal, result) = tried(&w).await;
    let v = verify::confirm(w.engine.ledger(), result).unwrap();
    assert_eq!(*v.value(), proposal);
    assert_eq!(v.evidence(), result);
}

#[tokio::test]
async fn confirm_rejects_other_grades() {
    for outcome in [
        Ok(FakeVerifier::outcome(Grade::Partial, "half")),
        Ok(FakeVerifier::outcome(Grade::Refuted, "no")),
        Err("crashed".to_string()), // recorded as Uncertain
    ] {
        let w = world_with(world::model(), outcome);
        let (_, _, result) = tried(&w).await;
        let err = verify::confirm(w.engine.ledger(), result).err().expect("must be refused");
        assert!(matches!(err, Error::NotVerified(_)), "{err:?}");
    }
}

#[tokio::test]
async fn confirm_rejects_non_verifier() {
    let w = world();
    let (_, proposal, _) = tried(&w).await;
    // The user writes a Confirmed-looking result by hand.
    let forged = w
        .engine
        .add(
            TrialResult {
                proposal,
                grade: Grade::Confirmed,
                observed: "trust me".into(),
                metrics: Default::default(),
            },
            &[proposal.raw()],
        )
        .unwrap();
    let err = verify::confirm(w.engine.ledger(), forged).err().expect("must be refused");
    assert!(matches!(err, Error::NotVerified(_)), "{err:?}");
}

#[tokio::test]
async fn confirm_rejects_the_wrong_kind_of_thing() {
    let w = world();
    let (_, proposal, _) = tried(&w).await;
    let err = verify::confirm(w.engine.ledger(), Id::<TrialResult>::from_raw(proposal.raw()))
        .err()
        .expect("must be refused");
    assert!(matches!(err, Error::WrongKind { .. }), "{err:?}");
}

#[tokio::test]
async fn establish_sets_head() {
    let w = world();
    let (_, proposal, result) = tried(&w).await;
    let part = w.engine.get(proposal).unwrap().part;
    let v = verify::confirm(w.engine.ledger(), result).unwrap();
    assert_eq!(w.engine.ledger().head(&format!("established/fridge/{part}")), None);
    w.engine.establish(v).unwrap();
    assert_eq!(
        w.engine.ledger().head(&format!("established/fridge/{part}")),
        Some(proposal.raw())
    );
}
