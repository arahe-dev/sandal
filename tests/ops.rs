mod common;
use common::world::{self, *};
use common::*;
use sandal::plug::{FakeModel, FakeVerifier};
use sandal::*;

/// Everything up to and including a proposal for source A and one for source B.
struct Pipeline {
    map: Id<Map>,
    study: Id<Study>,
}

async fn pipeline(w: &World) -> Pipeline {
    let map = make_map(w).await;
    let out = w
        .engine
        .run(&Cycle, CycleIn { map, k: 10, max_trials: 5, exclude_domains: vec![] })
        .await
        .unwrap();
    Pipeline { map, study: out.study }
}

fn kind_count(w: &World, kind: &str) -> usize {
    w.engine.ledger().things().iter().filter(|t| t.kind == kind).count()
}

fn thing_id(title: &str) -> ThingId {
    let item = world::items().into_iter().find(|i| i.title == title).unwrap();
    ThingId::compute("source_item", &serde_json::to_value(&item).unwrap(), &[])
}

#[tokio::test]
async fn back_never_sees_outcome() {
    let w = world();
    let ids = add_items(&w);
    w.engine.run(&Back, BackIn { thing: ids[0].raw() }).await.unwrap();
    let prompts = w.model.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains(A));
    assert!(!prompts[0].contains("OUTCOME-A"));
}

#[tokio::test]
async fn match_never_sees_outcome() {
    let w = world();
    let ids = add_items(&w);
    let map = make_map(&w).await;
    let origin = w.engine.run(&Back, BackIn { thing: ids[0].raw() }).await.unwrap();
    w.engine.run(&MatchOp, MatchIn { origin, map }).await.unwrap();
    for p in w.model.prompts() {
        assert!(!p.contains("OUTCOME"), "an outcome leaked into: {p}");
    }
}

#[tokio::test]
async fn carry_sees_outcome() {
    let w = world();
    let ids = add_items(&w);
    let map = make_map(&w).await;
    let origin = w.engine.run(&Back, BackIn { thing: ids[0].raw() }).await.unwrap();
    let ms = w.engine.run(&MatchOp, MatchIn { origin, map }).await.unwrap();
    w.engine.run(&Carry, CarryIn { m: ms[0] }).await.unwrap();
    let carry_prompt = w.model.prompts().into_iter().find(|p| p.contains("TASK: carry")).unwrap();
    assert!(carry_prompt.contains("OUTCOME-A removed fumes reliably"));
    assert!(carry_prompt.contains("check"), "lists the verifier's trial kinds");
}

#[tokio::test]
async fn match_drops_unknown_parts() {
    let w = world();
    let ids = add_items(&w);
    let map = make_map(&w).await;
    let origin = w.engine.run(&Back, BackIn { thing: ids[0].raw() }).await.unwrap();
    // The scripted reply names "odour" and also "no such part".
    let ms = w.engine.run(&MatchOp, MatchIn { origin, map }).await.unwrap();
    assert_eq!(ms.len(), 1);
    let m = w.engine.get(ms[0]).unwrap();
    assert_eq!(m.part, "odour");
    assert_eq!(kind_count(&w, "match"), 1);
}

#[tokio::test]
async fn match_drops_empty_shared_and_allows_empty_result() {
    let w = world();
    let ids = add_items(&w);
    let map = make_map(&w).await;
    // Source C's match reply is an empty list: valid, and appends nothing.
    let origin = w.engine.run(&Back, BackIn { thing: ids[2].raw() }).await.unwrap();
    let ms = w.engine.run(&MatchOp, MatchIn { origin, map }).await.unwrap();
    assert!(ms.is_empty());
    assert_eq!(kind_count(&w, "match"), 0);
}

#[tokio::test]
async fn carry_rejects_unknown_trial_kind() {
    let script: Vec<(String, String)> = world::script()
        .into_iter()
        .map(|(n, r)| (n, r.replace(r#""kind":"check""#, r#""kind":"nope""#)))
        .collect();
    let w = world_with(FakeModel::new(script), Ok(FakeVerifier::outcome(Grade::Confirmed, "ok")));
    let ids = add_items(&w);
    let map = make_map(&w).await;
    let origin = w.engine.run(&Back, BackIn { thing: ids[0].raw() }).await.unwrap();
    let ms = w.engine.run(&MatchOp, MatchIn { origin, map }).await.unwrap();
    let err = w.engine.run(&Carry, CarryIn { m: ms[0] }).await.unwrap_err();
    assert!(matches!(err, Error::BadModelOutput(_)), "{err:?}");
    assert_eq!(kind_count(&w, "proposal"), 0);
}

#[tokio::test]
async fn bad_json_retries_once() {
    let valid = r#"{"principles":["p"],"conditions":["c"]}"#;
    let r = rig(FakeModel::scripted(&[
        ("Your previous reply could not be parsed", valid),
        ("Title:", "garbage, not json"),
    ]));
    let x = r.engine.add(idea("an idea"), &[]).unwrap().raw();
    r.engine.run(&Back, BackIn { thing: x }).await.unwrap();
    assert_eq!(r.model.calls(), 2);
    let retry = &r.model.prompts()[1];
    assert!(retry.contains("could not be parsed"));

    let r = rig(FakeModel::scripted(&[("Title:", "garbage, not json")]));
    let x = r.engine.add(idea("an idea"), &[]).unwrap().raw();
    let err = r.engine.run(&Back, BackIn { thing: x }).await.unwrap_err();
    assert!(matches!(err, Error::BadModelOutput(_)), "{err:?}");
    assert_eq!(r.model.calls(), 2);
    assert_eq!(r.engine.ledger().things().iter().filter(|t| t.kind == "origin").count(), 0);
}

#[tokio::test]
async fn code_fences_are_stripped() {
    let fenced = "```json\n{\"principles\":[\"p\"],\"conditions\":[\"c\"]}\n```";
    let r = rig(FakeModel::scripted(&[("Title:", fenced)]));
    let x = r.engine.add(idea("an idea"), &[]).unwrap().raw();
    r.engine.run(&Back, BackIn { thing: x }).await.unwrap();
    assert_eq!(r.model.calls(), 1);
}

#[tokio::test]
async fn back_accepts_ideas_and_rejects_other_kinds() {
    let reply = r#"{"principles":["p"],"conditions":["c"]}"#;
    let r = rig(FakeModel::scripted(&[("Title:", reply)]));
    let idea_id = r.engine.add(idea("an idea"), &[]).unwrap().raw();
    r.engine.run(&Back, BackIn { thing: idea_id }).await.unwrap();
    // An origin is not something that can be backed.
    let origin = r
        .engine
        .ledger()
        .things()
        .into_iter()
        .find(|t| t.kind == "origin")
        .unwrap();
    let err = r.engine.run(&Back, BackIn { thing: origin.id }).await.unwrap_err();
    assert!(matches!(err, Error::WrongKind { .. }), "{err:?}");
}

#[tokio::test]
async fn forward_is_sorted_and_deep() {
    let w = world();
    let p = pipeline(&w).await;
    let study: Study = w.engine.get(p.study).unwrap();
    let proposal = study.order[0];
    let source = w.engine.get(proposal).unwrap().source;
    let result = w.engine.run(&Try, TryIn { proposal }).await.unwrap();

    let d = w.engine.run(&Forward, ForwardIn { thing: source, depth: 3 }).await.unwrap();
    // A source with an origin, a proposal and a result returns all three.
    let kinds: Vec<(&str, u8)> = d.iter().map(|x| (x.kind.as_str(), x.depth)).collect();
    assert!(kinds.contains(&("origin", 1)), "{kinds:?}");
    assert!(kinds.contains(&("proposal", 1)), "{kinds:?}");
    assert!(kinds.contains(&("match", 2)), "{kinds:?}");
    let res = d.iter().find(|x| x.id == result.raw()).expect("the trial result");
    assert_eq!(res.depth, 2);
    assert_eq!(res.grade, Some(Grade::Confirmed));
    // Sorted by (depth, id).
    let mut sorted = d.clone();
    sorted.sort_by_key(|x| (x.depth, x.id));
    assert_eq!(d, sorted);
    // Depth limits the walk.
    let shallow = w.engine.run(&Forward, ForwardIn { thing: source, depth: 1 }).await.unwrap();
    assert!(shallow.iter().all(|x| x.depth == 1));
    assert!(!shallow.iter().any(|x| x.id == result.raw()));
}

fn prop(e: &Engine, n: &str, parents: &[ThingId]) -> Id<Proposal> {
    let x = e.add(idea(n), &[]).unwrap().raw();
    let mut ps = vec![x];
    ps.extend_from_slice(parents);
    e.add(
        Proposal {
            source: x,
            origin: Id::from_raw(x),
            map: Id::from_raw(x),
            part: "p".into(),
            rebuilt: n.into(),
            root: n.into(),
            trial: TrialSpec { kind: "check".into(), params: Default::default(), description: "d".into() },
            if_right: "a".into(),
            if_wrong: "b".into(),
        },
        &ps,
    )
    .unwrap()
}

fn labels(e: &Engine, pairs: &[(Id<Proposal>, &str)]) -> Id<RootLabels> {
    e.add(
        RootLabels { labels: pairs.iter().map(|(p, l)| (*p, l.to_string())).collect() },
        &pairs.iter().map(|(p, _)| p.raw()).collect::<Vec<_>>(),
    )
    .unwrap()
}

#[tokio::test]
async fn mix_puts_largest_group_first() {
    let r = rig(FakeModel::scripted(&[]));
    let e = &r.engine;
    let ps: Vec<_> = (0..5).map(|n| prop(e, &format!("p{n}"), &[])).collect();
    let l = labels(e, &[(ps[0], "big"), (ps[1], "big"), (ps[2], "big"), (ps[3], "x"), (ps[4], "y")]);
    let id = e.run(&Mix, MixIn { labels: l, max_trials: 10 }).await.unwrap();
    let study = e.get(id).unwrap();
    assert_eq!(study.order.len(), 5);
    assert!([ps[0], ps[1], ps[2]].contains(&study.order[0]));
    // One from every group before the rest of the big one.
    assert_eq!(study.order[1], ps[3]);
    assert_eq!(study.order[2], ps[4]);
    // Groups are listed sorted by label.
    let names: Vec<&str> = study.groups.iter().map(|g| g.label.as_str()).collect();
    assert_eq!(names, vec!["big", "x", "y"]);
}

#[tokio::test]
async fn mix_truncates() {
    let r = rig(FakeModel::scripted(&[]));
    let e = &r.engine;
    let ps: Vec<_> = (0..4).map(|n| prop(e, &format!("p{n}"), &[])).collect();
    let l = labels(e, &[(ps[0], "a"), (ps[1], "a"), (ps[2], "b"), (ps[3], "c")]);
    let id = e.run(&Mix, MixIn { labels: l, max_trials: 2 }).await.unwrap();
    assert_eq!(e.get(id).unwrap().order.len(), 2);
}

#[tokio::test]
async fn mix_orders_within_a_group_by_missing_conditions() {
    let r = rig(FakeModel::scripted(&[]));
    let e = &r.engine;
    let mk = |n: &str, missing: usize| -> Id<Proposal> {
        let x = e.add(idea(&format!("origin {n}")), &[]).unwrap().raw();
        let m = e
            .add(
                Match {
                    origin: Id::from_raw(x),
                    map: Id::from_raw(x),
                    part: "p".into(),
                    shared: vec!["s".into()],
                    missing: (0..missing).map(|i| format!("m{i}")).collect(),
                    note: n.into(),
                },
                &[x],
            )
            .unwrap();
        prop(e, n, &[m.raw()])
    };
    let many = mk("many", 3);
    let none = mk("none", 0);
    let some = mk("some", 1);
    let l = labels(e, &[(many, "g"), (none, "g"), (some, "g")]);
    let id = e.run(&Mix, MixIn { labels: l, max_trials: 10 }).await.unwrap();
    assert_eq!(e.get(id).unwrap().order, vec![none, some, many]);
}

#[tokio::test]
async fn mix_is_deterministic() {
    let r = rig(FakeModel::scripted(&[("TASK: roots", r#"{"labels":["a","a","b"]}"#)]));
    let e = &r.engine;
    let ps: Vec<_> = (0..3).map(|n| prop(e, &format!("p{n}"), &[])).collect();
    let mut shuffled = vec![ps[2], ps[0], ps[1]];
    let l1 = e.run(&Roots, RootsIn { proposals: ps.clone() }).await.unwrap();
    let l2 = e.run(&Roots, RootsIn { proposals: shuffled.clone() }).await.unwrap();
    assert_eq!(l1, l2, "roots ignores input order");
    assert_eq!(r.model.calls(), 1, "and shares one memo entry");
    let s1 = e.run(&Mix, MixIn { labels: l1, max_trials: 5 }).await.unwrap();
    shuffled.reverse();
    let l3 = e.run(&Roots, RootsIn { proposals: shuffled }).await.unwrap();
    let s2 = e.run(&Mix, MixIn { labels: l3, max_trials: 5 }).await.unwrap();
    assert_eq!(s1, s2);
}

#[tokio::test]
async fn roots_rejects_a_wrong_label_count() {
    let r = rig(FakeModel::scripted(&[("TASK: roots", r#"{"labels":["only one"]}"#)]));
    let e = &r.engine;
    let ps: Vec<_> = (0..2).map(|n| prop(e, &format!("p{n}"), &[])).collect();
    let err = e.run(&Roots, RootsIn { proposals: ps }).await.unwrap_err();
    assert!(matches!(err, Error::BadModelOutput(_)), "{err:?}");
}

#[tokio::test]
async fn roots_of_nothing_is_empty_without_a_model_call() {
    let r = rig(FakeModel::scripted(&[]));
    let id = r.engine.run(&Roots, RootsIn { proposals: vec![] }).await.unwrap();
    assert!(r.engine.get(id).unwrap().labels.is_empty());
    assert_eq!(r.model.calls(), 0);
}

#[tokio::test]
async fn map_rejects_bad_maps() {
    for bad in [
        // one part
        r#"{"summary":"s","parts":[{"name":"a","description":"d","conditions":["c"]}]}"#,
        // duplicate names
        r#"{"summary":"s","parts":[{"name":"a","description":"d","conditions":["c"]},{"name":"a","description":"d","conditions":["c"]}]}"#,
        // a part without conditions
        r#"{"summary":"s","parts":[{"name":"a","description":"d","conditions":["c"]},{"name":"b","description":"d","conditions":[]}]}"#,
    ] {
        let r = rig(FakeModel::scripted(&[("TASK: map", bad)]));
        let err = r
            .engine
            .run(&MapOp, MapIn { name: "m".into(), problem: "p".into() })
            .await
            .unwrap_err();
        assert!(matches!(err, Error::BadModelOutput(_)), "{err:?}");
        assert!(r.engine.ledger().is_empty());
    }
}

#[tokio::test]
async fn try_records_uncertain_on_error() {
    let w = world_with(world::model(), Err("the verifier crashed".into()));
    let p = pipeline(&w).await;
    let proposal = w.engine.get(p.study).unwrap().order[0];
    let result = w.engine.run(&Try, TryIn { proposal }).await.unwrap();
    let r = w.engine.get(result).unwrap();
    assert_eq!(r.grade, Grade::Uncertain);
    assert!(r.observed.contains("the verifier crashed"));
    let thing = w.engine.thing(result.raw()).unwrap();
    assert!(matches!(thing.made_by, MadeBy::Verifier(_)));
}

#[tokio::test]
async fn read_separates_uncertain() {
    let w = world_with(world::model(), Err("timed out".into()));
    let p = pipeline(&w).await;
    let proposal = w.engine.get(p.study).unwrap().order[0];
    let source = w.engine.get(proposal).unwrap().source;
    w.engine.run(&Try, TryIn { proposal }).await.unwrap();
    w.engine.run(&Read, ReadIn { thing: source, map: p.map }).await.unwrap();
    let prompt = w.model.prompts().into_iter().rev().find(|p| p.contains("TASK: read")).unwrap();
    assert!(
        prompt.contains("## Trial results\n(none)\n\n## Trials that did not complete (not evidence either way)\n- Trial of:"),
        "{prompt}"
    );
}

#[tokio::test]
async fn read_shows_graded_results_as_evidence() {
    let w = world_with(world::model(), Ok(FakeVerifier::outcome(Grade::Refuted, "it did not work")));
    let p = pipeline(&w).await;
    let proposal = w.engine.get(p.study).unwrap().order[0];
    let source = w.engine.get(proposal).unwrap().source;
    w.engine.run(&Try, TryIn { proposal }).await.unwrap();
    w.engine.run(&Read, ReadIn { thing: source, map: p.map }).await.unwrap();
    let prompt = w.model.prompts().into_iter().rev().find(|p| p.contains("TASK: read")).unwrap();
    assert!(prompt.contains("## Trial results\n- Trial of:"));
    assert!(prompt.contains("Grade: refuted"));
    assert!(prompt.contains("did not complete (not evidence either way)\n(none)"));
    assert!(prompt.contains("Do not call the thing good or bad"));
}

#[tokio::test]
async fn notice_deduplicates_and_returns_the_first_k() {
    let w = world();
    let map = make_map(&w).await;
    let ids = w
        .engine
        .run(&Notice, NoticeIn { map, k: 2, exclude_domains: vec![] })
        .await
        .unwrap();
    assert_eq!(ids.len(), 2);
    assert_eq!(w.source.calls(), 1);
    let t = w.engine.thing(ids[0].raw()).unwrap();
    assert!(matches!(t.made_by, MadeBy::Source(_)));
    assert_eq!(ids[0].raw(), thing_id(A));
}
