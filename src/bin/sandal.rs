use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use sandal::bench::{self, BenchEnv};
use sandal::config::{build_model, Config, CONFIG_FILE, DEFAULT_CONFIG};
use sandal::plug::FakeVerifier;
use sandal::*;

#[derive(Parser)]
#[command(
    name = "sandal",
    version,
    about = "Carry ideas that worked in one place into a new problem, propose trials, and re-read conclusions as evidence arrives"
)]
struct Cli {
    /// The state directory.
    #[arg(long, global = true, default_value = ".sandal")]
    dir: PathBuf,
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

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The directory that holds `.sandal/`, against which relative source paths resolve.
fn project_root(dir: &Path) -> PathBuf {
    match dir.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

fn open(cli: &Cli, need_model: bool) -> Result<(Engine, Config)> {
    let cfg = Config::load(&cli.dir)?;
    let plugs = cfg.build_plugs(&project_root(&cli.dir), need_model)?;
    let engine = Engine::open(&cli.dir, plugs, cfg.budget())?;
    engine.set_max_tokens(cfg.model.max_tokens);
    Ok((engine, cfg))
}

fn footer(engine: &Engine) {
    let s = engine.stats();
    eprintln!(
        "model calls {}, world calls {}, hits {}, misses {}",
        s.model_calls, s.world_calls, s.hits, s.misses
    );
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

async fn run(cli: Cli) -> Result<()> {
    match &cli.command {
        Command::Init => init(&cli.dir),
        Command::Add { what } => {
            let (engine, _) = open(&cli, false)?;
            let id = match what {
                AddWhat::Source { title, body, domain, outcome } => engine
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
                AddWhat::Idea { title, body } => engine
                    .add(Idea { title: title.clone(), body: body.clone() }, &[])?
                    .raw(),
            };
            println!("{id}");
            Ok(())
        }
        Command::Map { name, problem_file } => {
            let problem = std::fs::read_to_string(problem_file)?;
            let (engine, _) = open(&cli, true)?;
            let map_id = engine
                .run(&MapOp, MapIn { name: name.clone(), problem })
                .await?;
            engine.set_head(&format!("map/{name}"), map_id.raw())?;
            let map = engine.get(map_id)?;
            println!("map/{name} -> {map_id}");
            println!("{}", map.summary);
            for p in &map.parts {
                println!("  {}: {}", p.name, p.description);
                for c in &p.conditions {
                    println!("    - {c}");
                }
            }
            footer(&engine);
            Ok(())
        }
        Command::Cycle { name, k, max_trials } => {
            let (engine, _) = open(&cli, true)?;
            let map = head_map(&engine, name)?;
            let out = engine
                .run(&Cycle, CycleIn { map, k: *k, max_trials: *max_trials, exclude_domains: Vec::new() })
                .await?;
            let study = engine.get(out.study)?;
            println!("study {}", out.study);
            for g in &study.groups {
                println!("group {:?}: {} proposal(s)", g.label, g.members.len());
            }
            println!("to try, in order:");
            for (i, id) in study.order.iter().enumerate() {
                let p = engine.get(*id)?;
                println!("  {}. {} [{}] {} (trial: {})", i + 1, id.raw().short(), p.part, clip(&p.rebuilt, 80), p.trial.kind);
            }
            if out.skipped > 0 {
                println!("{} item(s) skipped after a model or validation failure", out.skipped);
            }
            footer(&engine);
            Ok(())
        }
        Command::Try { proposal_id, force } => {
            let (engine, _) = open(&cli, false)?;
            let proposal = Id::<Proposal>::from_raw(engine.resolve(proposal_id)?);
            let input = TryIn { proposal };
            let result = if *force { engine.force(&Try, input).await? } else { engine.run(&Try, input).await? };
            let r = engine.get(result)?;
            println!("result {result}");
            println!("grade: {}", r.grade);
            println!("observed: {}", clip(&r.observed, 500));
            for (k, v) in &r.metrics {
                println!("metric {k} = {v}");
            }
            footer(&engine);
            Ok(())
        }
        Command::Establish { result_id } => {
            let (engine, _) = open(&cli, false)?;
            let result = Id::<TrialResult>::from_raw(engine.resolve(result_id)?);
            let verified = verify::confirm(engine.ledger(), result)?;
            let proposal = *verified.value();
            engine.establish(verified)?;
            println!("established proposal {proposal}");
            Ok(())
        }
        Command::Read { thing_id, map } => {
            let (engine, _) = open(&cli, true)?;
            let thing = engine.resolve(thing_id)?;
            let map = head_map(&engine, map)?;
            let id = engine.run(&Read, ReadIn { thing, map }).await?;
            let r = engine.get(id)?;
            println!("reading {id}");
            println!("{}", r.text);
            for u in &r.useful_for {
                println!("  useful for: {u}");
            }
            for c in &r.caveats {
                println!("  caveat: {c}");
            }
            footer(&engine);
            Ok(())
        }
        Command::Back { thing_id } => {
            let (engine, _) = open(&cli, true)?;
            let thing = engine.resolve(thing_id)?;
            let id = engine.run(&Back, BackIn { thing }).await?;
            let o = engine.get(id)?;
            println!("origin {id}");
            println!("principles:");
            for p in &o.principles {
                println!("  - {p}");
            }
            println!("conditions:");
            for c in &o.conditions {
                println!("  - {c}");
            }
            footer(&engine);
            Ok(())
        }
        Command::Forward { thing_id, depth } => {
            let (engine, _) = open(&cli, false)?;
            let thing = engine.resolve(thing_id)?;
            let ds = engine.run(&Forward, ForwardIn { thing, depth: *depth }).await?;
            print_tree(&engine, thing, &ds)
        }
        Command::Stale => {
            let (engine, _) = open(&cli, false)?;
            let stale = engine.stale();
            if stale.is_empty() {
                println!("nothing is stale");
            }
            for e in stale {
                println!("{:<8} {:<6} {}  {}", e.op, e.effect, hex_short(&e.key), clip(&e.input.to_string(), 80));
            }
            Ok(())
        }
        Command::Show { id } => {
            let (engine, _) = open(&cli, false)?;
            show(&engine, engine.resolve(id)?)
        }
        Command::Log { kind } => {
            let (engine, _) = open(&cli, false)?;
            for t in engine.ledger().things().iter().rev() {
                if kind.as_deref().is_some_and(|k| k != t.kind) {
                    continue;
                }
                println!("{} {:<13} {:<22} {}", t.id.short(), t.kind, t.made_by.to_string(), summary(t));
            }
            Ok(())
        }
        Command::Stats => {
            let (engine, _) = open(&cli, false)?;
            // DECISION: counters are per process, so a bare `sandal stats` starts
            // from zero. It also summarises what the ledger and memo hold.
            let s = engine.stats();
            println!("this run: model calls {}, world calls {}, hits {}, misses {}", s.model_calls, s.world_calls, s.hits, s.misses);
            let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
            for t in engine.ledger().things() {
                *kinds.entry(t.kind).or_default() += 1;
            }
            println!("ledger: {} things", engine.ledger().len());
            for (k, n) in kinds {
                println!("  {k:<13} {n}");
            }
            println!("heads: {}", engine.ledger().heads().len());
            println!("memo: {} entries, {} stale", engine.memo_entries().len(), engine.stale().len());
            Ok(())
        }
        Command::Bench { cases, n } => {
            let env = bench_env(&cli)?;
            let cases = bench::load_cases(cases)?;
            let report = bench::run(&cases, &env, *n).await?;
            print!("{}", bench::format_report(&report));
            std::fs::write("bench-report.json", serde_json::to_string_pretty(&report)?)?;
            println!("wrote bench-report.json");
            Ok(())
        }
        Command::BenchIncremental { cases } => {
            let env = bench_env(&cli)?;
            let cases = bench::load_cases(cases)?;
            let report = bench::run_incremental(&cases, &env).await?;
            print!("{}", bench::format_incremental(&report));
            println!("(with a real model, outputs vary between runs: compare the call counts, not the ids)");
            Ok(())
        }
    }
}

fn init(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    for f in [sandal::ledger::LEDGER_FILE, sandal::memo::MEMO_FILE] {
        std::fs::OpenOptions::new().create(true).append(true).open(dir.join(f))?;
    }
    let cfg = dir.join(CONFIG_FILE);
    if cfg.exists() {
        println!("kept the existing {}", cfg.display());
    } else {
        std::fs::write(&cfg, DEFAULT_CONFIG)?;
        println!("wrote {}", cfg.display());
    }
    println!("initialised {}", dir.display());
    Ok(())
}

fn bench_env(cli: &Cli) -> Result<BenchEnv> {
    let cfg = Config::load(&cli.dir)?;
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
        model: build_model(&cfg.model)?,
        judge: build_model(cfg.judge_model_config())?,
        verifier,
        budget: cfg.budget(),
        max_tokens: cfg.model.max_tokens,
    })
}

fn hex_short(key: &[u8; 32]) -> String {
    key.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

fn show(engine: &Engine, id: ThingId) -> Result<()> {
    let t = engine.thing(id)?;
    println!("id:       {}", t.id);
    println!("kind:     {}", t.kind);
    println!("made by:  {}", t.made_by);
    println!("at:       {}", t.at);
    println!("parents:");
    for p in &t.parents {
        println!("  {} {}", p.short(), engine.thing(*p).map(|x| x.kind).unwrap_or_default());
    }
    println!("children:");
    for c in engine.ledger().children(&id) {
        println!("  {} {}", c.short(), engine.thing(c).map(|x| x.kind).unwrap_or_default());
    }
    println!("payload:");
    println!("{}", serde_json::to_string_pretty(&t.payload)?);
    Ok(())
}

fn print_tree(engine: &Engine, root: ThingId, ds: &[Descendant]) -> Result<()> {
    let by_id: BTreeMap<ThingId, &Descendant> = ds.iter().map(|d| (d.id, d)).collect();
    let root_thing = engine.thing(root)?;
    println!("{} {} {}", root.short(), root_thing.kind, summary(&root_thing));
    let mut printed: HashSet<ThingId> = HashSet::new();
    fn walk(
        engine: &Engine,
        node: ThingId,
        depth: u8,
        by_id: &BTreeMap<ThingId, &Descendant>,
        printed: &mut HashSet<ThingId>,
    ) -> Result<()> {
        for child in engine.ledger().children(&node) {
            let Some(d) = by_id.get(&child) else { continue };
            // A thing reachable by several paths is shown once, under the first.
            if d.depth != depth + 1 || !printed.insert(child) {
                continue;
            }
            let t = engine.thing(child)?;
            let grade = d.grade.map(|g| format!(" [{g}]")).unwrap_or_default();
            println!("{}{} {}{} {}", "  ".repeat(depth as usize + 1), child.short(), t.kind, grade, summary(&t));
            walk(engine, child, depth + 1, by_id, printed)?;
        }
        Ok(())
    }
    walk(engine, root, 0, &by_id, &mut printed)?;
    if ds.is_empty() {
        println!("  (nothing yet)");
    }
    Ok(())
}
