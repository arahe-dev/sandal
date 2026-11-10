//! How model steps get answered: by the harness through handoff, or by a
//! program through `command`; and how config picks between them. SANDAL calls
//! no model API, so there is nothing here that touches the network.

mod common;
use common::*;
use sandal::config::{build_model, Config};
use sandal::plug::{handoff, CommandModel, HandoffModel};
use sandal::*;
use std::sync::Arc;

// ---- handoff ---------------------------------------------------------------------

fn handoff_engine(dir: &std::path::Path) -> Engine {
    let plugs = plugs(
        Arc::new(sandal::plug::FakeModel::new(vec![])),
        vec![],
        Arc::new(sandal::plug::FakeVerifier::new(vec![])),
    );
    let plugs = Plugs { model: Arc::new(HandoffModel::new(dir)), ..plugs };
    Engine::open(dir, plugs, Budget::default()).unwrap()
}

#[tokio::test]
async fn handoff_asks_then_reads_the_reply_back() {
    let dir = tempfile::tempdir().unwrap();
    let engine = handoff_engine(dir.path());
    let x = engine.add(idea("a thing that worked"), &[]).unwrap().raw();

    // First run: nothing answered, so it stops and says what it needs.
    let err = engine.run(&Back, BackIn { thing: x }).await.unwrap_err();
    assert!(matches!(err, Error::Pending(_)), "{err:?}");
    let list = handoff::pending(dir.path());
    assert_eq!(list.len(), 1);
    let prompt = std::fs::read_to_string(&list[0].prompt_file).unwrap();
    assert!(prompt.contains("TASK: back") && prompt.contains("a thing that worked"));
    assert!(prompt.contains(&list[0].response_file.display().to_string()));
    assert!(engine.ledger().things().iter().all(|t| t.kind != "origin"), "nothing was appended");

    // An empty reply file still counts as unanswered.
    std::fs::write(&list[0].response_file, "  \n").unwrap();
    assert!(engine.run(&Back, BackIn { thing: x }).await.is_err());

    // The reply arrives; the same command now completes, and then it is a memo hit.
    std::fs::write(&list[0].response_file, r#"{"principles":["p"],"conditions":["c"]}"#).unwrap();
    let origin = engine.run(&Back, BackIn { thing: x }).await.unwrap();
    assert_eq!(engine.get(origin).unwrap().principles, vec!["p"]);
    assert!(handoff::pending(dir.path()).is_empty());
    let before = engine.stats().misses;
    engine.run(&Back, BackIn { thing: x }).await.unwrap();
    assert_eq!(engine.stats().misses, before);
}

#[tokio::test]
async fn handoff_garbage_reply_asks_for_a_corrected_one() {
    let dir = tempfile::tempdir().unwrap();
    let engine = handoff_engine(dir.path());
    let x = engine.add(idea("an idea"), &[]).unwrap().raw();
    let _ = engine.run(&Back, BackIn { thing: x }).await;
    let first = handoff::pending(dir.path()).remove(0);
    std::fs::write(&first.response_file, "sorry, here is some prose").unwrap();
    // The single retry is a new prompt, so it is a new pending item, not a failure.
    let err = engine.run(&Back, BackIn { thing: x }).await.unwrap_err();
    assert!(matches!(err, Error::Pending(_)), "{err:?}");
    let list = handoff::pending(dir.path());
    assert_eq!(list.len(), 1);
    assert_ne!(list[0].key, first.key);
    assert!(std::fs::read_to_string(&list[0].prompt_file).unwrap().contains("could not be parsed"));
}

#[test]
fn pending_is_never_skipped_by_a_cycle() {
    let mut skipped = 0;
    let r: Vec<Result<u8>> = vec![Ok(1), Err(Error::Plug("x".into())), Err(Error::Pending("y".into()))];
    assert!(matches!(keep_ok(r, &mut skipped), Err(Error::Pending(_))));
    let r: Vec<Result<u8>> = vec![Ok(1), Err(Error::Plug("x".into()))];
    assert_eq!(keep_ok(r, &mut skipped).unwrap(), vec![1]);
    assert_eq!(skipped, 2);
}

#[tokio::test]
async fn answer_replies_to_a_pending_prompt_by_key_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let engine = handoff_engine(dir.path());
    let x = engine.add(idea("a thing"), &[]).unwrap().raw();
    let _ = engine.run(&Back, BackIn { thing: x }).await;
    let p = handoff::pending(dir.path()).remove(0);
    assert!(p.prompt.contains("TASK: back"), "the prompt text travels with the listing");

    assert!(handoff::answer(dir.path(), &p.key[..6], "  ").is_err(), "an empty reply is refused");
    assert!(handoff::answer(dir.path(), "zzzz", "{}").is_err(), "an unknown key is refused");
    let done = handoff::answer(dir.path(), &p.key[..6], "{\"principles\":[\"p\"],\"conditions\":[\"c\"]}\n").unwrap();
    assert_eq!(done.key, p.key);
    assert!(handoff::pending(dir.path()).is_empty());
    assert!(engine.run(&Back, BackIn { thing: x }).await.is_ok());
    // Nothing is pending any more, so answering again finds no prompt.
    assert!(handoff::answer(dir.path(), &p.key, "{}").is_err());
}

// ---- command ----------------------------------------------------------------------

fn echo_model() -> CommandModel {
    // A program that copies stdin to stdout.
    let (program, args) = if cfg!(windows) { ("findstr", vec!["^".to_string()]) } else { ("cat", vec![]) };
    CommandModel::new("echo", program, args, 20)
}

fn shell_model(script: &str, timeout: u64) -> CommandModel {
    if cfg!(windows) {
        CommandModel::new("sh", "cmd", vec!["/C".into(), script.into()], timeout)
    } else {
        CommandModel::new("sh", "sh", vec!["-c".into(), script.into()], timeout)
    }
}

#[tokio::test]
async fn command_model_pipes_the_prompt_through_stdin() {
    let m = echo_model();
    assert_eq!(m.id(), "command:echo");
    let reply = m.complete("SYSTEM TEXT", "PROMPT TEXT", 100).await.unwrap();
    assert!(reply.contains("SYSTEM TEXT") && reply.contains("PROMPT TEXT"), "{reply}");
    assert!(reply.find("SYSTEM TEXT").unwrap() < reply.find("PROMPT TEXT").unwrap());
}

#[tokio::test]
async fn command_model_reports_failures() {
    let err = shell_model("exit 3", 20).complete("s", "p", 10).await.unwrap_err();
    assert!(err.to_string().contains("exited"), "{err}");
    let err = CommandModel::new("", "definitely-not-a-program-xyz", vec![], 5).complete("s", "p", 10).await.unwrap_err();
    assert!(err.to_string().contains("could not start"), "{err}");
    let err = shell_model("rem nothing", 20).complete("s", "p", 10).await;
    if cfg!(windows) {
        assert!(err.is_err(), "no output is an error");
    }
    let script = if cfg!(windows) { "ping -n 5 127.0.0.1 >nul" } else { "sleep 20" };
    let err = shell_model(script, 1).complete("s", "p", 10).await.unwrap_err();
    assert!(err.to_string().contains("timed out"), "{err}");
}

#[tokio::test]
async fn a_command_model_can_drive_an_operation() {
    // The reply is whatever the program prints: here the contents of a file.
    let dir = tempfile::tempdir().unwrap();
    let reply = dir.path().join("reply.json");
    std::fs::write(&reply, r#"{"principles":["p"],"conditions":["c"]}"#).unwrap();
    let script = if cfg!(windows) { format!("type {}", reply.display()) } else { format!("cat '{}'", reply.display()) };
    let p = Plugs {
        model: Arc::new(shell_model(&script, 20)),
        sources: vec![],
        verifier: Arc::new(sandal::plug::FakeVerifier::new(vec![])),
    };
    let engine = Engine::open(dir.path(), p, Budget::default()).unwrap();
    let x = engine.add(idea("i"), &[]).unwrap().raw();
    let o = engine.run(&Back, BackIn { thing: x }).await.unwrap();
    assert_eq!(engine.get(o).unwrap().conditions, vec!["c"]);
}

// ---- config -----------------------------------------------------------------------

fn cfg(text: &str) -> Config {
    Config::parse(text).unwrap()
}

#[test]
fn config_picks_the_provider() {
    let dir = std::path::Path::new(".");
    assert_eq!(cfg("").model.provider, "handoff", "the default needs nothing");
    assert_eq!(build_model(&cfg("").model, dir).unwrap().id(), "handoff");

    let m = build_model(&cfg("[model]\nprovider = \"command\"\nname = \"x\"\ncommand = \"cat\"").model, dir).unwrap();
    assert_eq!(m.id(), "command:x");

    let bad = |t: &str| build_model(&cfg(t).model, dir).err().unwrap().to_string();
    assert!(bad("[model]\nprovider = \"command\"").contains("command is required"));
    // There are no vendor adapters: those names are refused with an explanation.
    for vendor in ["openai", "anthropic"] {
        let e = bad(&format!("[model]\nprovider = \"{vendor}\""));
        assert!(e.contains("unknown model provider") && e.contains("calls no model APIs"), "{e}");
    }
}

#[test]
fn environment_overrides_the_model_section() {
    // The only test that touches these variables.
    std::env::set_var("SANDAL_PROVIDER", "command");
    std::env::set_var("SANDAL_COMMAND", "my-harness");
    let c = cfg("[model]\nprovider = \"handoff\"").apply_env();
    std::env::remove_var("SANDAL_PROVIDER");
    std::env::remove_var("SANDAL_COMMAND");
    assert_eq!(c.model.provider, "command");
    assert_eq!(c.model.command.as_deref(), Some("my-harness"));
}

#[test]
fn the_default_config_parses() {
    let c = cfg(sandal::config::DEFAULT_CONFIG);
    assert_eq!(c.model.provider, "handoff");
    assert_eq!(c.verifier.templates.len(), 1);
}

