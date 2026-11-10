use sandal::bench::{self, BenchCase, BenchEnv};
use sandal::plug::{FakeModel, FakeVerifier};
use sandal::*;
use std::path::Path;
use std::sync::Arc;

fn fixtures() -> Vec<BenchCase> {
    bench::load_cases(&Path::new(env!("CARGO_MANIFEST_DIR")).join("bench/fixtures.jsonl")).unwrap()
}

/// Replies for the annealing case: only its true source gets a usable origin, a
/// match and a proposal; the distractors' origins fail and are skipped by the cycle.
fn script(case: &BenchCase) -> Vec<(String, String)> {
    let pairs: Vec<(String, String)> = vec![
        ("TASK: map".into(), r#"{"summary":"s","parts":[
            {"name":"search","description":"how the search moves","conditions":["Greedy moves get stuck"]},
            {"name":"layout","description":"the chip layout","conditions":["Wiring length is the cost"]}]}"#.into()),
        ("TASK: roots".into(), r#"{"labels":["one assumption"]}"#.into()),
        ("TASK: bench_plain".into(), "SET LATER".into()),
        ("Principles of the earlier idea:\n- BENCH-PRINCIPLE".into(), r#"{"matches":[
            {"part":"search","shared":["Greedy moves get stuck"],"missing":[],"note":"n"}]}"#.into()),
        ("What made it work:\n- BENCH-PRINCIPLE".into(), r#"{"rebuilt":"Accept some worse swaps while hot, then cool slowly","root":"escaping needs noise",
            "trial":{"kind":"check","params":{},"description":"Run the check"},"if_right":"shorter wiring","if_wrong":"no change"}"#.into()),
        ("## Trial results\n- Trial of:".into(), r#"{"text":"WITH EVIDENCE","useful_for":["search"],"caveats":[]}"#.into()),
        ("TASK: read".into(), r#"{"text":"WITHOUT EVIDENCE","useful_for":["search"],"caveats":[]}"#.into()),
        (format!("Title: {}", case.true_source.title), r#"{"principles":["BENCH-PRINCIPLE: noise lets a search escape"],"conditions":["Greedy moves get stuck"]}"#.into()),
    ];
    pairs
}

fn plain_reply(case: &BenchCase) -> String {
    let order = bench::shuffled_sources(case);
    let idx = order.iter().position(|s| s.title == case.true_source.title).unwrap();
    format!(r#"{{"proposals":[{{"source_index":{idx},"rebuilt":"Accept some worse swaps, then cool slowly"}}]}}"#)
}

fn env(model: Arc<FakeModel>, judge: Arc<FakeModel>) -> BenchEnv {
    BenchEnv {
        model,
        judge,
        verifier: Arc::new(FakeVerifier::new(vec![("check", Ok(FakeVerifier::outcome(Grade::Confirmed, "ok")))])),
        budget: Budget::default(),
        max_tokens: 1000,
    }
}

fn scripted_env(case: &BenchCase) -> (BenchEnv, Arc<FakeModel>, Arc<FakeModel>) {
    let mut s = script(case);
    for (needle, reply) in s.iter_mut() {
        if needle == "TASK: bench_plain" {
            *reply = plain_reply(case);
        }
    }
    let model = Arc::new(FakeModel::new(s));
    let judge = Arc::new(FakeModel::scripted(&[("TASK: bench_judge", r#"{"covered":[true,true,false]}"#)]));
    (env(model.clone(), judge.clone()), model, judge)
}

#[test]
fn fixtures_parse() {
    let cases = fixtures();
    assert_eq!(cases.len(), 3);
    for c in &cases {
        assert!(!c.id.is_empty());
        assert_eq!(c.distractors.len(), 4);
        assert_eq!(c.truth.key_points.len(), 3);
        assert!(c.true_source.outcome.is_some());
    }
}

#[test]
fn shuffle_is_seeded_by_the_case_id() {
    let cases = fixtures();
    let a = bench::shuffled_sources(&cases[0]);
    assert_eq!(a, bench::shuffled_sources(&cases[0]));
    assert_eq!(a.len(), 5);
    assert!(a.iter().any(|s| s.title == cases[0].true_source.title));
    // The order is not just "true source first" for every case.
    let firsts: Vec<bool> = cases
        .iter()
        .map(|c| bench::shuffled_sources(c)[0].title == c.true_source.title)
        .collect();
    assert!(firsts.iter().any(|f| !f), "{firsts:?}");
}

#[tokio::test]
async fn sandal_arm_scores_hit() {
    let cases = fixtures();
    let (env, _, _) = scripted_env(&cases[0]);
    let report = bench::run(&cases[..1], &env, 3).await.unwrap();
    let r = &report.cases[0];
    assert!(r.sandal.source_hit, "the true source is the source of a proposal");
    assert!((r.sandal.coverage - 2.0 / 3.0).abs() < 1e-9);
    assert!(r.sandal.model_calls > r.plain.model_calls, "the pipeline costs more calls");
    assert_eq!(report.sandal.hits, 1);
    assert!(bench::format_report(&report).contains("TOTAL (1 cases)"));
}

#[tokio::test]
async fn plain_arm_runs_one_call() {
    let cases = fixtures();
    let (env, model, judge) = scripted_env(&cases[0]);
    let report = bench::run(&cases[..1], &env, 3).await.unwrap();
    let r = &report.cases[0];
    assert_eq!(r.plain.model_calls, 1);
    assert!(r.plain.source_hit);
    assert!((r.plain.coverage - 2.0 / 3.0).abs() < 1e-9);
    // One plain prompt in all, and the judge was called once per arm.
    let plain_prompts = model.prompts().iter().filter(|p| p.contains("TASK: bench_plain")).count();
    assert_eq!(plain_prompts, 1);
    assert_eq!(judge.calls(), 2);
    // The plain arm sees every source with its outcome.
    let prompt = model.prompts().into_iter().find(|p| p.contains("TASK: bench_plain")).unwrap();
    for s in std::iter::once(&cases[0].true_source).chain(&cases[0].distractors) {
        assert!(prompt.contains(&s.title));
        assert!(prompt.contains(s.outcome.as_deref().unwrap()));
    }
}

#[tokio::test]
async fn judge_is_blind() {
    let cases = fixtures();
    let (env, _, judge) = scripted_env(&cases[0]);
    bench::run(&cases[..1], &env, 3).await.unwrap();
    let prompts = judge.prompts();
    assert_eq!(prompts.len(), 2);
    for p in prompts {
        let p = p.to_lowercase();
        assert!(!p.contains("sandal"), "{p}");
        assert!(!p.contains("plain"), "{p}");
    }
}

#[tokio::test]
async fn both_arms_get_the_same_sources_in_the_same_order() {
    let cases = fixtures();
    let (env, model, _) = scripted_env(&cases[0]);
    bench::run(&cases[..1], &env, 3).await.unwrap();
    let order = bench::shuffled_sources(&cases[0]);
    let prompt = model.prompts().into_iter().find(|p| p.contains("TASK: bench_plain")).unwrap();
    let positions: Vec<usize> = order.iter().map(|s| prompt.find(&s.title).unwrap()).collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "{positions:?}");
}

#[tokio::test]
async fn incremental_matches_full() {
    let cases = fixtures();
    let (env, _, _) = scripted_env(&cases[0]);
    let report = bench::run_incremental(&cases[..1], &env).await.unwrap();
    let c = &report.cases[0];
    assert!(!c.no_proposals);
    // Refresh and full routes end with identical reading ids...
    assert!(c.same_reading_ids);
    // ...and refresh used fewer Read calls: one stale reading against five.
    assert_eq!(c.refresh_calls, 1);
    assert_eq!(c.full_calls, 5);
    assert!(c.refresh_calls < c.full_calls);
    assert!(bench::format_incremental(&report).contains("TOTAL"));
}

#[tokio::test]
async fn incremental_reports_a_case_with_no_proposals() {
    let cases = fixtures();
    // A model that maps the problem but finds no matches: nothing to try.
    let model = Arc::new(FakeModel::scripted(&[
        ("TASK: map", r#"{"summary":"s","parts":[
            {"name":"a","description":"d","conditions":["c"]},{"name":"b","description":"d","conditions":["c"]}]}"#),
        ("TASK: roots", r#"{"labels":[]}"#),
        ("TASK: read", r#"{"text":"t","useful_for":[],"caveats":[]}"#),
    ]));
    let judge = Arc::new(FakeModel::scripted(&[]));
    let report = bench::run_incremental(&cases[..1], &env(model, judge)).await.unwrap();
    assert!(report.cases[0].no_proposals);
}
