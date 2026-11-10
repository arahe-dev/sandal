//! Fixture 4 (bench/fixture4_pairing.json) is the acceptance test for `Pair`:
//! a known weakness and a new delivery channel must be paired, and `Mix` must
//! order the same proposals as it did before `Pair` existed.

mod common;
use common::*;
use sandal::plug::FakeModel;
use sandal::*;
use std::collections::BTreeMap;

fn fixture4() -> serde_json::Value {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bench/fixture4_pairing.json"),
    )
    .unwrap();
    serde_json::from_str(&text).unwrap()
}

/// A proposal with the given rebuilt text and root. Its origin and map are
/// stand-ins: neither `Mix` nor `Pair` reads them.
fn proposal(e: &Engine, rebuilt: &str, root: &str) -> Id<Proposal> {
    let x = e.add(idea(rebuilt), &[]).unwrap().raw();
    e.add(
        Proposal {
            source: x,
            origin: Id::from_raw(x),
            map: Id::from_raw(x),
            part: "p".into(),
            rebuilt: rebuilt.into(),
            root: root.into(),
            trial: TrialSpec { kind: "check".into(), params: Default::default(), description: "d".into() },
            if_right: "a".into(),
            if_wrong: "b".into(),
        },
        &[x],
    )
    .unwrap()
}

/// Label the given proposals by root and run `Mix` on them.
async fn mix_of(e: &Engine, labelled: Vec<(Id<Proposal>, String)>) -> Id<Study> {
    let labels = e
        .add(
            RootLabels { labels: labelled.clone() },
            &labelled.iter().map(|(p, _)| p.raw()).collect::<Vec<_>>(),
        )
        .unwrap();
    e.run(&Mix, MixIn { labels, max_trials: 10 }).await.unwrap()
}

/// Build the fixture's proposals, and the root each one belongs to.
fn fixture_proposals(e: &Engine, fx: &serde_json::Value) -> Vec<(Id<Proposal>, String)> {
    let mut out = Vec::new();
    for (root, items) in fx["roots"].as_object().unwrap() {
        for item in items.as_array().unwrap() {
            out.push((proposal(e, item.as_str().unwrap(), root), root.clone()));
        }
    }
    out
}

#[tokio::test]
async fn pair_joins_a_known_weakness_with_a_new_channel() {
    let fx = fixture4();
    let r = rig(FakeModel::scripted(&[]));
    let e = &r.engine;
    let labelled = fixture_proposals(e, &fx);
    let root_of: BTreeMap<ThingId, String> =
        labelled.iter().map(|(p, l)| (p.raw(), l.clone())).collect();
    let study_id = mix_of(e, labelled).await;
    let study = e.get(study_id).unwrap();

    // Mix is unchanged: the bigger root goes first, then the other root's
    // head, then the rest of the bigger root.
    let order: Vec<&str> = study.order.iter().map(|p| root_of[&p.raw()].as_str()).collect();
    assert_eq!(order, vec!["known weakness", "new delivery channel", "known weakness"]);

    // Pair joins the two roots: the best member of each, as the first two of Mix's order.
    let pairing = e.run(&Pair, PairIn { study: study_id, max_pairs: 5 }).await.unwrap();
    let pairs = e.get(pairing).unwrap().pairs;
    assert_eq!(pairs, vec![(study.order[0], study.order[1])]);
    let got = [&pairs[0].0, &pairs[0].1].map(|p| root_of[&p.raw()].clone());
    let expected: Vec<String> = fx["expected_pair"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(got.to_vec(), expected);
}

#[tokio::test]
async fn pair_respects_max_pairs_and_needs_two_roots() {
    let fx = fixture4();
    let r = rig(FakeModel::scripted(&[]));
    let e = &r.engine;
    let study_id = mix_of(e, fixture_proposals(e, &fx)).await;

    let none = e.run(&Pair, PairIn { study: study_id, max_pairs: 0 }).await.unwrap();
    assert!(e.get(none).unwrap().pairs.is_empty());

    // One root only: nothing complementary to pair with.
    let p = proposal(e, "only one", "one root");
    let study = mix_of(e, vec![(p, "one root".into())]).await;
    let pairing = e.run(&Pair, PairIn { study, max_pairs: 5 }).await.unwrap();
    assert!(e.get(pairing).unwrap().pairs.is_empty());
}
