//! The bench is how SANDAL is judged. It replays historical transfers with the
//! transfer itself hidden and compares the pipeline against the same model
//! asked once.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::ctx::{parse_json, retry_prompt};
use crate::engine::{Budget, Engine, Plugs};
use crate::error::{Error, Result};
use crate::id::{sha256, Id, ThingId};
use crate::ops::{
    render, Cycle, CycleIn, MapIn, MapOp, Read, ReadIn, Try, TryIn, SYSTEM,
};
use crate::plug::{FakeSource, Model, Verifier};
use crate::thing::{Payload, Reading, SourceItem};

// ---- cases --------------------------------------------------------------------

/// One JSON object per line in a cases file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchCase {
    pub id: String,
    pub target: Target,
    pub true_source: SourceItem,
    pub distractors: Vec<SourceItem>,
    pub truth: Truth,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Target {
    pub name: String,
    /// The target problem as it stood before the transfer.
    pub problem: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Truth {
    /// What was actually carried over and how.
    pub summary: String,
    pub key_points: Vec<String>,
}

pub fn load_cases(path: &Path) -> Result<Vec<BenchCase>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| Error::Plug(format!("cannot read cases file {}: {e}", path.display())))?;
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| {
            serde_json::from_str(l)
                .map_err(|e| Error::Plug(format!("{}: line {}: {e}", path.display(), i + 1)))
        })
        .collect()
}

impl BenchCase {
    /// Id of the true source once it is in a ledger.
    pub fn true_source_id(&self) -> ThingId {
        source_id(&self.true_source)
    }
}

fn source_id(item: &SourceItem) -> ThingId {
    let payload = serde_json::to_value(item).expect("a source item serialises");
    ThingId::compute(SourceItem::KIND, &payload, &[])
}

/// The true source and the distractors, in an order shuffled with a seed taken
/// from the case id. Both arms get this same order.
pub fn shuffled_sources(case: &BenchCase) -> Vec<SourceItem> {
    let mut items: Vec<SourceItem> = std::iter::once(case.true_source.clone())
        .chain(case.distractors.iter().cloned())
        .collect();
    let digest = sha256(&[case.id.as_bytes()]);
    let mut state = u64::from_be_bytes(digest[..8].try_into().expect("8 bytes"));
    // splitmix64, then Fisher-Yates.
    let mut next = move || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    for i in (1..items.len()).rev() {
        let j = (next() % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
    items
}

// ---- environment -----------------------------------------------------------------

/// What the bench runs against.
#[derive(Clone)]
pub struct BenchEnv {
    /// The model under test.
    pub model: Arc<dyn Model>,
    /// The model that judges coverage. Set separately in config, and should be
    /// at least as strong as the model under test.
    pub judge: Arc<dyn Model>,
    /// Supplies the trial kinds that `carry` chooses from. No trial is run by `bench`.
    pub verifier: Arc<dyn Verifier>,
    pub budget: Budget,
    pub max_tokens: u32,
}

/// A temporary directory, removed when dropped. (`tempfile` is a dev-dependency only.)
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Result<TempDir> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "sandal-bench-{}-{nanos}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir)?;
        Ok(TempDir(dir))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fresh_engine(env: &BenchEnv, dir: &TempDir, order: &[SourceItem]) -> Result<Engine> {
    let plugs = Plugs {
        model: env.model.clone(),
        sources: vec![Arc::new(FakeSource::new("bench", order.to_vec()))],
        verifier: env.verifier.clone(),
    };
    let engine = Engine::open(&dir.0, plugs, env.budget.clone())?;
    engine.set_max_tokens(env.max_tokens);
    Ok(engine)
}

// ---- report ------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchReport {
    pub cases: Vec<CaseResult>,
    pub sandal: ArmTotals,
    pub plain: ArmTotals,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CaseResult {
    pub id: String,
    pub sandal: ArmScore,
    pub plain: ArmScore,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArmScore {
    pub source_hit: bool,
    pub coverage: f64,
    pub model_calls: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArmTotals {
    pub hits: u32,
    pub mean_coverage: f64,
    pub model_calls: u64,
}

fn totals(scores: &[&ArmScore]) -> ArmTotals {
    let n = scores.len().max(1) as f64;
    ArmTotals {
        hits: scores.iter().filter(|s| s.source_hit).count() as u32,
        mean_coverage: scores.iter().map(|s| s.coverage).sum::<f64>() / n,
        model_calls: scores.iter().map(|s| s.model_calls).sum(),
    }
}

/// One row per case and a totals row for each arm. Cost sits beside quality so
/// that the trade can be judged.
pub fn format_report(r: &BenchReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{:<28} {:>13} {:>9} {:>6}   {:>13} {:>9} {:>6}\n",
        "case", "sandal hit", "coverage", "calls", "plain hit", "coverage", "calls"
    ));
    for c in &r.cases {
        out.push_str(&format!(
            "{:<28} {:>13} {:>9.2} {:>6}   {:>13} {:>9.2} {:>6}\n",
            c.id,
            c.sandal.source_hit,
            c.sandal.coverage,
            c.sandal.model_calls,
            c.plain.source_hit,
            c.plain.coverage,
            c.plain.model_calls
        ));
    }
    out.push_str(&format!(
        "{:<28} {:>13} {:>9.2} {:>6}   {:>13} {:>9.2} {:>6}\n",
        format!("TOTAL ({} cases)", r.cases.len()),
        format!("{}/{}", r.sandal.hits, r.cases.len()),
        r.sandal.mean_coverage,
        r.sandal.model_calls,
        format!("{}/{}", r.plain.hits, r.cases.len()),
        r.plain.mean_coverage,
        r.plain.model_calls
    ));
    out
}

// ---- model helper outside the engine -------------------------------------------

/// `ask_json` for the arms that run outside an engine: same fence stripping and
/// the same single retry. Returns the value and how many model calls it took.
async fn ask_json_direct<T: DeserializeOwned>(
    model: &Arc<dyn Model>,
    prompt: &str,
    max_tokens: u32,
) -> Result<(T, u64)> {
    let first = model.complete(SYSTEM, prompt, max_tokens).await?;
    match parse_json::<T>(&first) {
        Ok(v) => Ok((v, 1)),
        Err(e) => {
            let second = model.complete(SYSTEM, &retry_prompt(prompt, &e), max_tokens).await?;
            parse_json::<T>(&second)
                .map(|v| (v, 2))
                .map_err(|e2| Error::BadModelOutput(format!("could not parse model reply as JSON: {e2}")))
        }
    }
}

// ---- the two arms ------------------------------------------------------------------

struct ArmOutput {
    hit: bool,
    texts: Vec<String>,
    calls: u64,
}

async fn sandal_arm(case: &BenchCase, order: &[SourceItem], env: &BenchEnv, n: usize) -> Result<ArmOutput> {
    // Fresh temporary ledger, and a source that returns the true source plus the distractors.
    let dir = TempDir::new()?;
    let engine = fresh_engine(env, &dir, order)?;
    let map = engine
        .run(&MapOp, MapIn { name: case.target.name.clone(), problem: case.target.problem.clone() })
        .await?;
    let out = engine
        .run(
            &Cycle,
            CycleIn { map, k: order.len(), max_trials: n, exclude_domains: Vec::new() },
        )
        .await?;
    let study = engine.get(out.study)?;
    let true_id = case.true_source_id();
    let mut hit = false;
    let mut texts = Vec::new();
    for id in study.order.iter().take(n) {
        let p = engine.get(*id)?;
        hit |= p.source == true_id;
        texts.push(p.rebuilt);
    }
    Ok(ArmOutput { hit, texts, calls: engine.stats().model_calls })
}

#[derive(Deserialize)]
struct PlainReply {
    proposals: Vec<PlainProposal>,
}

#[derive(Deserialize)]
struct PlainProposal {
    source_index: usize,
    rebuilt: String,
}

const PLAIN_TEMPLATE: &str = include_str!("prompts/bench_plain.txt");
const JUDGE_TEMPLATE: &str = include_str!("prompts/bench_judge.txt");

async fn plain_arm(case: &BenchCase, order: &[SourceItem], env: &BenchEnv, n: usize) -> Result<ArmOutput> {
    let sources = order
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let outcome = s.outcome.as_deref().unwrap_or("not recorded");
            format!("{i}. {}\n   Domain: {}\n   {}\n   Outcome: {outcome}", s.title, s.domain, s.body)
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let prompt = render(
        PLAIN_TEMPLATE,
        &[("n", &n.to_string()), ("problem", &case.target.problem), ("sources", &sources)],
    );
    let (reply, calls): (PlainReply, u64) = ask_json_direct(&env.model, &prompt, env.max_tokens).await?;
    let true_id = case.true_source_id();
    let mut hit = false;
    let mut texts = Vec::new();
    for p in reply.proposals.into_iter().take(n) {
        let item = order.get(p.source_index).ok_or_else(|| {
            Error::BadModelOutput(format!("source_index {} is out of range", p.source_index))
        })?;
        hit |= source_id(item) == true_id;
        texts.push(p.rebuilt);
    }
    Ok(ArmOutput { hit, texts, calls })
}

#[derive(Deserialize)]
struct JudgeReply {
    covered: Vec<bool>,
}

/// Fraction of key points the judge says the arm's rebuilt texts cover. The
/// judge prompt never says which arm it is scoring.
async fn coverage(case: &BenchCase, texts: &[String], env: &BenchEnv) -> Result<f64> {
    if texts.is_empty() || case.truth.key_points.is_empty() {
        return Ok(0.0);
    }
    let key_points = case
        .truth
        .key_points
        .iter()
        .enumerate()
        .map(|(i, k)| format!("{}. {k}", i + 1))
        .collect::<Vec<_>>()
        .join("\n");
    let texts = texts
        .iter()
        .enumerate()
        .map(|(i, t)| format!("{}. {t}", i + 1))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = render(JUDGE_TEMPLATE, &[("key_points", &key_points), ("texts", &texts)]);
    let (reply, _): (JudgeReply, u64) = ask_json_direct(&env.judge, &prompt, env.max_tokens).await?;
    if reply.covered.len() != case.truth.key_points.len() {
        return Err(Error::BadModelOutput(format!(
            "judge returned {} verdicts for {} key points",
            reply.covered.len(),
            case.truth.key_points.len()
        )));
    }
    Ok(reply.covered.iter().filter(|c| **c).count() as f64 / reply.covered.len() as f64)
}

async fn score(arm: ArmOutput, case: &BenchCase, env: &BenchEnv) -> Result<ArmScore> {
    Ok(ArmScore {
        source_hit: arm.hit,
        coverage: coverage(case, &arm.texts, env).await?,
        model_calls: arm.calls,
    })
}

/// Run both arms on every case. `n` is how many proposals each arm may make.
pub async fn run(cases: &[BenchCase], env: &BenchEnv, n: usize) -> Result<BenchReport> {
    let mut results = Vec::new();
    for case in cases {
        let ctx = |e: Error| Error::Plug(format!("case {}: {e}", case.id));
        let order = shuffled_sources(case);
        let sandal = sandal_arm(case, &order, env, n).await.map_err(ctx)?;
        let plain = plain_arm(case, &order, env, n).await.map_err(ctx)?;
        results.push(CaseResult {
            id: case.id.clone(),
            sandal: score(sandal, case, env).await.map_err(ctx)?,
            plain: score(plain, case, env).await.map_err(ctx)?,
        });
    }
    let sandal = totals(&results.iter().map(|r| &r.sandal).collect::<Vec<_>>());
    let plain = totals(&results.iter().map(|r| &r.plain).collect::<Vec<_>>());
    Ok(BenchReport { cases: results, sandal, plain })
}

// ---- the incremental measurement ----------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IncrementalCase {
    pub id: String,
    /// Model calls the second round of readings used after the trial.
    pub refresh_calls: u64,
    /// Model calls the same readings used in a fresh ledger.
    pub full_calls: u64,
    /// True when both routes ended with identical reading ids. Under a real
    /// model outputs vary between runs, so only the call counts are comparable.
    pub same_reading_ids: bool,
    /// The cycle produced no proposal, so there was nothing to try.
    pub no_proposals: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IncrementalReport {
    pub cases: Vec<IncrementalCase>,
}

pub fn format_incremental(r: &IncrementalReport) -> String {
    let mut out = format!("{:<28} {:>14} {:>11} {:>10}\n", "case", "refresh calls", "full calls", "same ids");
    for c in &r.cases {
        let note = if c.no_proposals { "  (no proposals, nothing tried)" } else { "" };
        out.push_str(&format!(
            "{:<28} {:>14} {:>11} {:>10}{note}\n",
            c.id, c.refresh_calls, c.full_calls, c.same_reading_ids
        ));
    }
    let refresh: u64 = r.cases.iter().map(|c| c.refresh_calls).sum();
    let full: u64 = r.cases.iter().map(|c| c.full_calls).sum();
    out.push_str(&format!("{:<28} {:>14} {:>11}\n", "TOTAL", refresh, full));
    out
}

async fn read_all(engine: &Engine, sources: &[ThingId], map: Id<crate::thing::Map>) -> Result<Vec<Id<Reading>>> {
    let mut out = Vec::new();
    for s in sources {
        out.push(engine.run(&Read, ReadIn { thing: *s, map }).await?);
    }
    Ok(out)
}

/// Checks the second claim: after new evidence, refreshing the readings costs
/// fewer model calls than computing them from scratch, and lands on the same readings.
pub async fn run_incremental(cases: &[BenchCase], env: &BenchEnv) -> Result<IncrementalReport> {
    let mut out = Vec::new();
    for case in cases {
        let ctx = |e: Error| Error::Plug(format!("case {}: {e}", case.id));
        out.push(incremental_case(case, env).await.map_err(ctx)?);
    }
    Ok(IncrementalReport { cases: out })
}

async fn incremental_case(case: &BenchCase, env: &BenchEnv) -> Result<IncrementalCase> {
    let order = shuffled_sources(case);
    let sources: Vec<ThingId> = order.iter().map(source_id).collect();
    let map_in = || MapIn { name: case.target.name.clone(), problem: case.target.problem.clone() };
    let cycle_in = |map| CycleIn { map, k: order.len(), max_trials: 3, exclude_domains: Vec::new() };

    // Refresh route: cycle, read every source, try the first proposal, read every source again.
    let dir = TempDir::new()?;
    let engine = fresh_engine(env, &dir, &order)?;
    let map = engine.run(&MapOp, map_in()).await?;
    let cycle = engine.run(&Cycle, cycle_in(map)).await?;
    let first = engine.get(cycle.study)?.order.first().copied();
    read_all(&engine, &sources, map).await?;
    let Some(first) = first else {
        return Ok(IncrementalCase {
            id: case.id.clone(),
            refresh_calls: 0,
            full_calls: 0,
            same_reading_ids: true,
            no_proposals: true,
        });
    };
    engine.run(&Try, TryIn { proposal: first }).await?;
    let before = engine.stats().model_calls;
    let refreshed = read_all(&engine, &sources, map).await?;
    let refresh_calls = engine.stats().model_calls - before;

    // Full route: a fresh ledger gets the cycle and the trial first, and only then the readings.
    let dir2 = TempDir::new()?;
    let engine2 = fresh_engine(env, &dir2, &order)?;
    let map2 = engine2.run(&MapOp, map_in()).await?;
    let cycle2 = engine2.run(&Cycle, cycle_in(map2)).await?;
    let first2 = engine2.get(cycle2.study)?.order.first().copied();
    if let Some(p) = first2 {
        engine2.run(&Try, TryIn { proposal: p }).await?;
    }
    let before = engine2.stats().model_calls;
    let full = read_all(&engine2, &sources, map2).await?;
    let full_calls = engine2.stats().model_calls - before;

    Ok(IncrementalCase {
        id: case.id.clone(),
        refresh_calls,
        full_calls,
        same_reading_ids: refreshed == full,
        no_proposals: false,
    })
}
