use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use sandal::bench::{self, BenchEnv};
use sandal::config::{build_model, Config, CONFIG_FILE, DEFAULT_CONFIG};
use sandal::plug::{handoff, FakeVerifier};
use sandal::*;
use serde_json::{json, Value};

/// Exit codes, stable for scripts and agents.
const EXIT_ERROR: u8 = 1;
/// Waiting on model replies (the handoff provider): answer them and run again.
const EXIT_PENDING: u8 = 3;

#[derive(Parser)]
#[command(
    name = "sandal",
    version,
    about = "Carry ideas that worked in one place into a new problem, propose trials, and re-read conclusions as evidence arrives",
    after_help = "Exit codes: 0 done, 1 error, 3 waiting on model replies (provider = \"handoff\"; see AGENTS.md)."
)]
struct Cli {
    /// The state directory.
    #[arg(long, global = true, default_value = ".sandal")]
    dir: PathBuf,
    /// Print one JSON document on stdout instead of text (errors too).
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create .sandal/ with empty files and a default config
    Init,
    /// Append a source item or an idea
    Add {
        #[command(subcommand)]
        what: AddWhat,
    },
    /// Break a problem into parts and set the head map/<name>
    Map {
        name: String,
        #[arg(long)]
        problem_file: PathBuf,
    },
    /// Run a cycle on the head map and print the study
    Cycle {
        name: String,
        #[arg(long, default_value_t = 8)]
        k: usize,
        #[arg(long, default_value_t = 5)]
        max_trials: usize,
    },
    /// Run a proposal's trial and print the result
    Try {
        proposal_id: String,
        /// Run it again even if a result is stored
        #[arg(long)]
        force: bool,
    },
    /// Confirm a trial result and mark its proposal as established
    Establish { result_id: String },
    /// Say what a thing is useful for, given a map
    Read {
        thing_id: String,
        #[arg(long)]
        map: String,
    },
    /// Extract what made a thing work
    Back { thing_id: String },
    /// Print what a thing led to, as a tree
    Forward {
        thing_id: String,
        #[arg(long, default_value_t = 3)]
        depth: u8,
    },
    /// List stored results whose dependencies have moved
    Stale,
    /// Print one thing with its parents and children
    Show { id: String },
    /// List things, newest first
    Log {
        #[arg(long)]
        kind: Option<String>,
    },
    /// Print calls, hits and misses for this run, and what the ledger holds
    Stats,
    /// List model prompts that are waiting for a reply (provider = "handoff")
    Pending,
    /// Run the bench: the pipeline against the same model asked once
    Bench {
        cases: PathBuf,
        #[arg(long, default_value_t = 3)]
        n: usize,
    },
    /// Check that refreshing readings after new evidence costs fewer calls than recomputing
    BenchIncremental { cases: PathBuf },
}

#[derive(Subcommand)]
enum AddWhat {
    /// Append a SourceItem
    Source {
        #[arg(long)]
        title: String,
        #[arg(long)]
        body: String,
        #[arg(long)]
        domain: String,
        #[arg(long)]
        outcome: Option<String>,
    },
    /// Append an Idea
    Idea {
        #[arg(long)]
        title: String,
        #[arg(long)]
        body: String,
    },
}

/// What a command produced: the same facts as JSON and as text.
struct Out {
    json: Value,
    text: String,
}

macro_rules! line {
    ($t:expr) => { { let _ = writeln!($t); } };
    ($t:expr, $($a:tt)*) => { { let _ = writeln!($t, $($a)*); } };
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli).await {
        Ok(out) => {
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&out.json).unwrap_or_default());
            } else {
                print!("{}", out.text);
            }
            ExitCode::SUCCESS
        }
        Err(e) => fail(&cli, e),
    }
}

fn fail(cli: &Cli, e: Error) -> ExitCode {
    if let Error::Pending(_) = e {
        let list = handoff::pending(&cli.dir);
        if cli.json {
            let doc = json!({
                "ok": false,
                "status": "pending",
                "message": "model replies are needed: write each reply to its response_file, then run the same command again",
                "pending": list,
            });
            println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
        } else {
            eprintln!("waiting on {} model reply(ies). For each prompt below, write ONLY the reply to its response file, then run the same command again:", list.len());
            for p in &list {
                eprintln!("  prompt:   {}\n  response: {}", p.prompt_file.display(), p.response_file.display());
            }
            if list.is_empty() {
                eprintln!("  ({e})");
            }
        }
        return ExitCode::from(EXIT_PENDING);
    }
    if cli.json {
        let doc = json!({ "ok": false, "status": "error", "error": e.to_string() });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
    } else {
        eprintln!("error: {e}");
    }
    ExitCode::from(EXIT_ERROR)
}

/// The directory that holds `.sandal/`, against which relative source paths resolve.
fn project_root(dir: &Path) -> PathBuf {
    match dir.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

fn load_config(cli: &Cli) -> Result<Config> {
    Ok(Config::load(&cli.dir)?.apply_env())
}

fn open(cli: &Cli, need_model: bool) -> Result<Engine> {
    let cfg = load_config(cli)?;
    let plugs = cfg.build_plugs(&project_root(&cli.dir), &cli.dir, need_model)?;
    let engine = Engine::open(&cli.dir, plugs, cfg.budget())?;
    engine.set_max_tokens(cfg.model.max_tokens);
    Ok(engine)
}

fn stats_json(engine: &Engine) -> Value {
    let s = engine.stats();
    json!({ "model_calls": s.model_calls, "world_calls": s.world_calls, "hits": s.hits, "misses": s.misses })
}

/// Attach this run's counters: to the JSON document, and as a line on stderr.
fn with_stats(engine: &Engine, cli: &Cli, mut out: Out) -> Out {
    out.json["stats"] = stats_json(engine);
    if !cli.json {
        let s = engine.stats();
        eprintln!(
            "model calls {}, world calls {}, hits {}, misses {}",
            s.model_calls, s.world_calls, s.hits, s.misses
        );
    }
    out
}

fn head_map(engine: &Engine, name: &str) -> Result<Id<Map>> {
    engine
        .ledger()
        .head(&format!("map/{name}"))
        .map(Id::from_raw)
        .ok_or_else(|| Error::NotFound(format!("no map named {name:?}: run `sandal map {name} --problem-file <path>`")))
}

fn clip(s: &str, n: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= n {
        one
    } else {
        format!("{}…", one.chars().take(n).collect::<String>())
    }
}

/// A short human label for a thing, from whichever text field it has.
fn summary(thing: &Thing) -> String {
    for field in ["title", "name", "rebuilt", "text", "note", "observed", "part", "label"] {
        if let Some(s) = thing.payload.get(field).and_then(|v| v.as_str()) {
            return clip(s, 70);
        }
    }
    String::new()
}

async fn run(cli: &Cli) -> Result<Out> {
    match &cli.command {
        Command::Init => init(&cli.dir),
        Command::Add { what } => {
            let engine = open(cli, false)?;
            let (id, kind) = match what {
                AddWhat::Source { title, body, domain, outcome } => (
                    engine
                        .add(
                            SourceItem {
                                title: title.clone(),
                                body: body.clone(),
                                domain: domain.clone(),
                                outcome: outcome.clone(),
                                url: None,
                            },
                            &[],
                        )?
                        .raw(),
                    "source_item",
                ),
                AddWhat::Idea { title, body } => (
                    engine.add(Idea { title: title.clone(), body: body.clone() }, &[])?.raw(),
                    "idea",
                ),
            };
            Ok(Out { json: json!({ "ok": true, "id": id, "kind": kind }), text: format!("{id}\n") })
        }
        Command::Map { name, problem_file } => {
            let problem = std::fs::read_to_string(problem_file)?;
            let engine = open(cli, true)?;
            let map_id = engine.run(&MapOp, MapIn { name: name.clone(), problem }).await?;
            engine.set_head(&format!("map/{name}"), map_id.raw())?;
            let map = engine.get(map_id)?;
            let mut t = String::new();
            line!(t, "map/{name} -> {map_id}");
            line!(t, "{}", map.summary);
            for p in &map.parts {
                line!(t, "  {}: {}", p.name, p.description);
                for c in &p.conditions {
                    line!(t, "    - {c}");
                }
            }
            let json = json!({ "ok": true, "name": name, "map": map_id, "summary": map.summary, "parts": map.parts });
            Ok(with_stats(&engine, cli, Out { json, text: t }))
        }
        Command::Cycle { name, k, max_trials } => {
            let engine = open(cli, true)?;
            let map = head_map(&engine, name)?;
            let out = engine
                .run(&Cycle, CycleIn { map, k: *k, max_trials: *max_trials, exclude_domains: Vec::new() })
                .await?;
            let study = engine.get(out.study)?;
            let mut t = String::new();
            line!(t, "study {}", out.study);
            for g in &study.groups {
                line!(t, "group {:?}: {} proposal(s)", g.label, g.members.len());
            }
            line!(t, "to try, in order:");
            let mut order = Vec::new();
            for (i, id) in study.order.iter().enumerate() {
                let p = engine.get(*id)?;
                line!(t, "  {}. {} [{}] {} (trial: {})", i + 1, id.raw().short(), p.part, clip(&p.rebuilt, 80), p.trial.kind);
                order.push(json!({
                    "proposal": id, "part": p.part, "rebuilt": p.rebuilt, "root": p.root,
                    "trial_kind": p.trial.kind, "trial": p.trial.description,
                    "if_right": p.if_right, "if_wrong": p.if_wrong,
                }));
            }
            if out.skipped > 0 {
                line!(t, "{} item(s) skipped after a model or validation failure", out.skipped);
            }
            let json = json!({
                "ok": true, "study": out.study, "skipped": out.skipped,
                "groups": study.groups, "order": order,
            });
            Ok(with_stats(&engine, cli, Out { json, text: t }))
        }
        Command::Try { proposal_id, force } => {
            let engine = open(cli, false)?;
            let proposal = Id::<Proposal>::from_raw(engine.resolve(proposal_id)?);
            let input = TryIn { proposal };
            let result = if *force { engine.force(&Try, input).await? } else { engine.run(&Try, input).await? };
            let r = engine.get(result)?;
            let mut t = String::new();
            line!(t, "result {result}");
            line!(t, "grade: {}", r.grade);
            line!(t, "observed: {}", clip(&r.observed, 500));
            for (k, v) in &r.metrics {
                line!(t, "metric {k} = {v}");
            }
            let json = json!({
                "ok": true, "result": result, "proposal": proposal, "grade": r.grade,
                "observed": r.observed, "metrics": r.metrics,
            });
            Ok(with_stats(&engine, cli, Out { json, text: t }))
        }
        Command::Establish { result_id } => {
            let engine = open(cli, false)?;
            let result = Id::<TrialResult>::from_raw(engine.resolve(result_id)?);
            let verified = verify::confirm(engine.ledger(), result)?;
            let proposal = *verified.value();
            engine.establish(verified)?;
            Ok(Out {
                json: json!({ "ok": true, "proposal": proposal, "evidence": result }),
                text: format!("established proposal {proposal}\n"),
            })
        }
        Command::Read { thing_id, map } => {
            let engine = open(cli, true)?;
            let thing = engine.resolve(thing_id)?;
            let map = head_map(&engine, map)?;
            let id = engine.run(&Read, ReadIn { thing, map }).await?;
            let r = engine.get(id)?;
            let mut t = String::new();
            line!(t, "reading {id}");
            line!(t, "{}", r.text);
            for u in &r.useful_for {
                line!(t, "  useful for: {u}");
            }
            for c in &r.caveats {
                line!(t, "  caveat: {c}");
            }
            let json = json!({
                "ok": true, "reading": id, "of": thing, "text": r.text,
                "useful_for": r.useful_for, "caveats": r.caveats,
            });
            Ok(with_stats(&engine, cli, Out { json, text: t }))
        }
        Command::Back { thing_id } => {
            let engine = open(cli, true)?;
            let thing = engine.resolve(thing_id)?;
            let id = engine.run(&Back, BackIn { thing }).await?;
            let o = engine.get(id)?;
            let mut t = String::new();
            line!(t, "origin {id}");
            line!(t, "principles:");
            for p in &o.principles {
                line!(t, "  - {p}");
            }
            line!(t, "conditions:");
            for c in &o.conditions {
                line!(t, "  - {c}");
            }
            let json = json!({
                "ok": true, "origin": id, "of": thing,
                "principles": o.principles, "conditions": o.conditions,
            });
            Ok(with_stats(&engine, cli, Out { json, text: t }))
        }
        Command::Forward { thing_id, depth } => {
            let engine = open(cli, false)?;
            let thing = engine.resolve(thing_id)?;
            let ds = engine.run(&Forward, ForwardIn { thing, depth: *depth }).await?;
            let text = tree(&engine, thing, &ds)?;
            Ok(Out { json: json!({ "ok": true, "thing": thing, "descendants": ds }), text })
        }
        Command::Stale => {
            let engine = open(cli, false)?;
            let stale = engine.stale();
            let mut t = String::new();
            if stale.is_empty() {
                line!(t, "nothing is stale");
            }
            for e in &stale {
                line!(t, "{:<8} {:<6} {}  {}", e.op, e.effect, hex_short(&e.key), clip(&e.input.to_string(), 80));
            }
            let items: Vec<Value> = stale
                .iter()
                .map(|e| json!({ "op": e.op, "effect": e.effect, "key": hex::encode(e.key), "input": e.input }))
                .collect();
            Ok(Out { json: json!({ "ok": true, "stale": items }), text: t })
        }
        Command::Show { id } => {
            let engine = open(cli, false)?;
            show(&engine, engine.resolve(id)?)
        }
        Command::Log { kind } => {
            let engine = open(cli, false)?;
            let mut t = String::new();
            let mut items = Vec::new();
            for th in engine.ledger().things().iter().rev() {
                if kind.as_deref().is_some_and(|k| k != th.kind) {
                    continue;
                }
                line!(t, "{} {:<13} {:<22} {}", th.id.short(), th.kind, th.made_by.to_string(), summary(th));
                items.push(json!({
                    "id": th.id, "kind": th.kind, "made_by": th.made_by, "summary": summary(th), "at": th.at,
                }));
            }
            Ok(Out { json: json!({ "ok": true, "things": items }), text: t })
        }
        Command::Stats => {
            let engine = open(cli, false)?;
            // DECISION: counters are per process, so a bare `sandal stats` starts
            // from zero. It also summarises what the ledger and memo hold.
            let s = engine.stats();
            let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
            for th in engine.ledger().things() {
                *kinds.entry(th.kind).or_default() += 1;
            }
            let mut t = String::new();
            line!(t, "this run: model calls {}, world calls {}, hits {}, misses {}", s.model_calls, s.world_calls, s.hits, s.misses);
            line!(t, "ledger: {} things", engine.ledger().len());
            for (k, n) in &kinds {
                line!(t, "  {k:<13} {n}");
            }
            line!(t, "heads: {}", engine.ledger().heads().len());
            line!(t, "memo: {} entries, {} stale", engine.memo_entries().len(), engine.stale().len());
            let json = json!({
                "ok": true, "this_run": stats_json(&engine), "things": engine.ledger().len(), "kinds": kinds,
                "heads": engine.ledger().heads().len(),
                "memo": { "entries": engine.memo_entries().len(), "stale": engine.stale().len() },
            });
            Ok(Out { json, text: t })
        }
        Command::Pending => {
            let list = handoff::pending(&cli.dir);
            let mut t = String::new();
            if list.is_empty() {
                line!(t, "nothing is waiting for a reply");
            }
            for p in &list {
                line!(t, "prompt:   {}\nresponse: {}", p.prompt_file.display(), p.response_file.display());
            }
            Ok(Out { json: json!({ "ok": true, "pending": list }), text: t })
        }
        Command::Bench { cases, n } => {
            let env = bench_env(cli)?;
            let cases = bench::load_cases(cases)?;
            let report = bench::run(&cases, &env, *n).await?;
            std::fs::write("bench-report.json", serde_json::to_string_pretty(&report)?)?;
            let mut text = bench::format_report(&report);
            text.push_str("wrote bench-report.json\n");
            Ok(Out { json: json!({ "ok": true, "report": report }), text })
        }
        Command::BenchIncremental { cases } => {
            let env = bench_env(cli)?;
            let cases = bench::load_cases(cases)?;
            let report = bench::run_incremental(&cases, &env).await?;
            let mut text = bench::format_incremental(&report);
            text.push_str("(with a real model, outputs vary between runs: compare the call counts, not the ids)\n");
            Ok(Out { json: json!({ "ok": true, "report": report }), text })
        }
    }
}

fn init(dir: &Path) -> Result<Out> {
    std::fs::create_dir_all(dir)?;
    for f in [sandal::ledger::LEDGER_FILE, sandal::memo::MEMO_FILE] {
        std::fs::OpenOptions::new().create(true).append(true).open(dir.join(f))?;
    }
    let cfg = dir.join(CONFIG_FILE);
    let mut t = String::new();
    let written = !cfg.exists();
    if written {
        std::fs::write(&cfg, DEFAULT_CONFIG)?;
        line!(t, "wrote {}", cfg.display());
    } else {
        line!(t, "kept the existing {}", cfg.display());
    }
    line!(t, "initialised {}", dir.display());
    Ok(Out {
        json: json!({ "ok": true, "dir": dir, "config": cfg, "config_written": written }),
        text: t,
    })
}

fn bench_env(cli: &Cli) -> Result<BenchEnv> {
    let cfg = load_config(cli)?;
    let verifier = cfg.build_verifier()?;
    // DECISION: with no verifier templates in config, `carry` would have no
    // trial kind to choose and every proposal would be refused. The bench runs
    // no trials, so give it one placeholder kind that always reports an error.
    let verifier: Arc<dyn Verifier> = if verifier.kinds().is_empty() {
        Arc::new(FakeVerifier::new(vec![("manual", Err("no verifier configured".to_string()))]))
    } else {
        Arc::new(verifier)
    };
    Ok(BenchEnv {
        model: build_model(&cfg.model, &cli.dir)?,
        judge: build_model(cfg.judge_model_config(), &cli.dir)?,
        verifier,
        budget: cfg.budget(),
        max_tokens: cfg.model.max_tokens,
    })
}

fn hex_short(key: &[u8; 32]) -> String {
    key.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

fn show(engine: &Engine, id: ThingId) -> Result<Out> {
    let th = engine.thing(id)?;
    let kind_of = |x: &ThingId| engine.thing(*x).map(|t| t.kind).unwrap_or_default();
    let children = engine.ledger().children(&id);
    let mut t = String::new();
    line!(t, "id:       {}", th.id);
    line!(t, "kind:     {}", th.kind);
    line!(t, "made by:  {}", th.made_by);
    line!(t, "at:       {}", th.at);
    line!(t, "parents:");
    for p in &th.parents {
        line!(t, "  {} {}", p.short(), kind_of(p));
    }
    line!(t, "children:");
    for c in &children {
        line!(t, "  {} {}", c.short(), kind_of(c));
    }
    line!(t, "payload:");
    line!(t, "{}", serde_json::to_string_pretty(&th.payload)?);
    let describe = |ids: &[ThingId]| -> Vec<Value> {
        ids.iter().map(|x| json!({ "id": x, "kind": kind_of(x) })).collect()
    };
    let json = json!({
        "ok": true, "thing": th,
        "parents": describe(&th.parents), "children": describe(&children),
    });
    Ok(Out { json, text: t })
}

fn tree(engine: &Engine, root: ThingId, ds: &[Descendant]) -> Result<String> {
    let by_id: BTreeMap<ThingId, &Descendant> = ds.iter().map(|d| (d.id, d)).collect();
    let root_thing = engine.thing(root)?;
    let mut t = String::new();
    line!(t, "{} {} {}", root.short(), root_thing.kind, summary(&root_thing));
    let mut printed: HashSet<ThingId> = HashSet::new();
    fn walk(
        engine: &Engine,
        node: ThingId,
        depth: u8,
        by_id: &BTreeMap<ThingId, &Descendant>,
        printed: &mut HashSet<ThingId>,
        t: &mut String,
    ) -> Result<()> {
        for child in engine.ledger().children(&node) {
            let Some(d) = by_id.get(&child) else { continue };
            // A thing reachable by several paths is shown once, under the first.
            if d.depth != depth + 1 || !printed.insert(child) {
                continue;
            }
            let th = engine.thing(child)?;
            let grade = d.grade.map(|g| format!(" [{g}]")).unwrap_or_default();
            line!(t, "{}{} {}{} {}", "  ".repeat(depth as usize + 1), child.short(), th.kind, grade, summary(&th));
            walk(engine, child, depth + 1, by_id, printed, t)?;
        }
        Ok(())
    }
    walk(engine, root, 0, &by_id, &mut printed, &mut t)?;
    if ds.is_empty() {
        line!(t, "  (nothing yet)");
    }
    Ok(t)
}
