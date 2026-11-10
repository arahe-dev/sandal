#![allow(dead_code)]
//! A small scripted world used by the operation tests and by the example: a
//! target problem, four source items, and a model scripted to answer each
//! operation's prompt. It depends only on the `sandal` crate.

use std::sync::Arc;

use sandal::plug::{FakeModel, FakeSource, FakeVerifier};
use sandal::*;

pub const PROBLEM: &str = "Keep a shared office fridge from smelling.";

pub fn item(title: &str, domain: &str, outcome: &str) -> SourceItem {
    SourceItem {
        title: title.into(),
        body: format!("{title}: how it was done."),
        domain: domain.into(),
        outcome: Some(outcome.into()),
        url: None,
    }
}

pub const A: &str = "Charcoal filters in gas masks";
pub const B: &str = "Salt curing of fish";
pub const C: &str = "Bell towers";
pub const D: &str = "Failing item";

pub fn items() -> Vec<SourceItem> {
    vec![
        item(A, "military", "OUTCOME-A removed fumes reliably"),
        item(B, "food", "OUTCOME-B kept fish for months"),
        item(C, "architecture", "OUTCOME-C rang across the valley"),
        item(D, "misc", "OUTCOME-D unknown"),
    ]
}

pub fn script() -> Vec<(String, String)> {
    let pairs: &[(&str, &str)] = &[
        ("TASK: map", r#"{"summary":"A fridge that smells","parts":[
            {"name":"odour","description":"smell in the air","conditions":["Air passes through the material"]},
            {"name":"waste","description":"food left to rot","conditions":["Scraps are left for days"]}]}"#),
        ("TASK: roots", r#"{"labels":["shared assumption","shared assumption"]}"#),
        ("Principles of the earlier idea:\n- PRINCIPLE-A", r#"{"matches":[
            {"part":"odour","shared":["Air passes through the material"],"missing":["A sealed housing forces the air through"],"note":"close"},
            {"part":"no such part","shared":["x"],"missing":[],"note":"dropped"}]}"#),
        ("Principles of the earlier idea:\n- PRINCIPLE-B", r#"{"matches":[
            {"part":"waste","shared":["Scraps are left for days"],"missing":[],"note":"close"}]}"#),
        ("Principles of the earlier idea:\n- PRINCIPLE-C", r#"{"matches":[]}"#),
        ("What made it work:\n- PRINCIPLE-A", r#"{"rebuilt":"Hang a charcoal pad in the fridge","root":"odour is airborne",
            "trial":{"kind":"check","params":{},"description":"Run the check"},"if_right":"smell gone","if_wrong":"smell stays"}"#),
        ("What made it work:\n- PRINCIPLE-B", r#"{"rebuilt":"Salt the scraps","root":"scraps cause the odour",
            "trial":{"kind":"check","params":{},"description":"Run the check"},"if_right":"smell gone","if_wrong":"smell stays"}"#),
        ("## Trial results\n- Trial of:", r#"{"text":"EVIDENCE reading","useful_for":["odour"],"caveats":["one trial"]}"#),
        ("TASK: read", r#"{"text":"NO-EVIDENCE reading","useful_for":["odour"],"caveats":["nothing tried yet"]}"#),
        (&format!("Title: {A}"), r#"{"principles":["PRINCIPLE-A: charcoal adsorbs odour"],"conditions":["Air passes through the material"]}"#),
        (&format!("Title: {B}"), r#"{"principles":["PRINCIPLE-B: salt stops rot"],"conditions":["Scraps are left for days"]}"#),
        (&format!("Title: {C}"), r#"{"principles":["PRINCIPLE-C: bells resonate"],"conditions":["A heavy bell hangs free"]}"#),
        (&format!("Title: {D}"), "this is not json at all"),
    ];
    pairs.iter().map(|(n, r)| (n.to_string(), r.to_string())).collect()
}

pub fn model() -> FakeModel {
    FakeModel::new(script())
}

pub struct World {
    pub dir: tempfile::TempDir,
    pub engine: Engine,
    pub model: Arc<FakeModel>,
    pub source: Arc<FakeSource>,
    pub verifier: Arc<FakeVerifier>,
}

/// An engine over the scripted world. `outcome` is what the `check` trial returns.
pub fn world_with(model: FakeModel, outcome: std::result::Result<TrialOutcome, String>) -> World {
    let dir = tempfile::tempdir().unwrap();
    let model = Arc::new(model);
    let source = Arc::new(FakeSource::new("fake", items()));
    let verifier = Arc::new(FakeVerifier::new(vec![("check", outcome)]));
    let plugs = Plugs {
        model: model.clone(),
        sources: vec![source.clone()],
        verifier: verifier.clone(),
    };
    let engine = Engine::open(dir.path(), plugs, Budget::default()).unwrap();
    World { dir, engine, model, source, verifier }
}

pub fn world() -> World {
    world_with(model(), Ok(FakeVerifier::outcome(Grade::Confirmed, "it worked")))
}

/// Run `map` on the problem.
pub async fn make_map(w: &World) -> Id<Map> {
    w.engine
        .run(&MapOp, MapIn { name: "fridge".into(), problem: PROBLEM.into() })
        .await
        .unwrap()
}

/// Add every source item as a user-entered thing and return their ids in order.
pub fn add_items(w: &World) -> Vec<Id<SourceItem>> {
    items().into_iter().map(|i| w.engine.add(i, &[]).unwrap()).collect()
}
