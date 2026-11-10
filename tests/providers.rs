//! The model providers: handoff, command, OpenAI-compatible, Anthropic, and
//! how config picks between them. No real network: the HTTP adapters talk to a
//! tiny server on localhost.

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
    assert!(bad("[model]\nprovider = \"openai\"").contains("name is required"));
    assert!(bad("[model]\nprovider = \"anthropic\"").contains("name is required"));
    assert!(bad("[model]\nprovider = \"bogus\"").contains("unknown model provider"));
}

#[cfg(feature = "openai")]
#[test]
fn openai_needs_a_key_only_for_the_real_endpoint() {
    let dir = std::path::Path::new(".");
    // A key variable that is certainly unset.
    let t = |extra: &str| {
        cfg(&format!("[model]\nprovider = \"openai\"\nname = \"m\"\napi_key_env = \"SANDAL_TEST_NO_SUCH_KEY\"\n{extra}"))
    };
    assert!(build_model(&t("").model, dir).err().unwrap().to_string().contains("SANDAL_TEST_NO_SUCH_KEY"));
    // A local server takes no key.
    assert!(build_model(&t("base_url = \"http://localhost:11434/v1\"").model, dir).is_ok());
}

#[test]
fn environment_overrides_the_model_section() {
    // The only test that touches these variables.
    std::env::set_var("SANDAL_PROVIDER", "command");
    std::env::set_var("SANDAL_MODEL", "from-env");
    std::env::set_var("SANDAL_BASE_URL", "http://example.invalid/v1");
    let c = cfg("[model]\nprovider = \"handoff\"\nname = \"from-file\"").apply_env();
    std::env::remove_var("SANDAL_PROVIDER");
    std::env::remove_var("SANDAL_MODEL");
    std::env::remove_var("SANDAL_BASE_URL");
    assert_eq!(c.model.provider, "command");
    assert_eq!(c.model.name, "from-env");
    assert_eq!(c.model.base_url.as_deref(), Some("http://example.invalid/v1"));
}

#[test]
fn the_default_config_parses() {
    let c = cfg(sandal::config::DEFAULT_CONFIG);
    assert_eq!(c.model.provider, "handoff");
    assert_eq!(c.verifier.templates.len(), 1);
}

// ---- HTTP adapters, against a server on localhost ----------------------------------

#[cfg(any(feature = "openai", feature = "anthropic"))]
mod http {
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Answer one request with the given status and body. Returns the base URL
    /// and a handle to the raw request text the server saw.
    pub async fn serve_once(status: u16, body: &str) -> (String, Arc<Mutex<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(String::new()));
        let seen2 = seen.clone();
        let body = body.to_string();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = sock.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).into_owned();
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let len = text[..head_end]
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()))
                        .unwrap_or(0);
                    if buf.len() >= head_end + 4 + len {
                        break;
                    }
                }
            }
            *seen2.lock().unwrap() = String::from_utf8_lossy(&buf).into_owned();
            let reply = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(reply.as_bytes()).await.unwrap();
            let _ = sock.shutdown().await;
        });
        (format!("http://{addr}"), seen)
    }

    pub fn body_of(request: &str) -> serde_json::Value {
        serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap()
    }
}

#[cfg(feature = "openai")]
#[tokio::test]
async fn openai_compatible_adapter_speaks_chat_completions() {
    use sandal::plug::openai::OpenAiModel;
    let reply = r#"{"choices":[{"message":{"role":"assistant","content":"hello back"}}]}"#;
    let (base, seen) = http::serve_once(200, reply).await;
    let m = OpenAiModel::new("some-local-model", &format!("{base}/v1/"), Some("sk-test".into()), "max_tokens");
    assert_eq!(m.id(), "some-local-model");
    assert_eq!(m.complete("be brief", "say hi", 123).await.unwrap(), "hello back");

    let req = seen.lock().unwrap().clone();
    assert!(req.starts_with("POST /v1/chat/completions "), "{req}");
    assert!(req.to_lowercase().contains("authorization: bearer sk-test"));
    let body = http::body_of(&req);
    assert_eq!(body["model"], "some-local-model");
    assert_eq!(body["max_tokens"], 123);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][0]["content"], "be brief");
    assert_eq!(body["messages"][1]["role"], "user");
    assert_eq!(body["messages"][1]["content"], "say hi");
}

#[cfg(feature = "openai")]
#[tokio::test]
async fn openai_adapter_can_rename_the_token_field_and_skip_the_key() {
    use sandal::plug::openai::OpenAiModel;
    let reply = r#"{"choices":[{"message":{"content":"ok"}}]}"#;
    let (base, seen) = http::serve_once(200, reply).await;
    let m = OpenAiModel::new("m", &base, None, "max_completion_tokens");
    m.complete("s", "p", 77).await.unwrap();
    let req = seen.lock().unwrap().clone();
    assert!(req.starts_with("POST /chat/completions "));
    assert!(!req.to_lowercase().contains("authorization"));
    let body = http::body_of(&req);
    assert_eq!(body["max_completion_tokens"], 77);
    assert!(body.get("max_tokens").is_none());
}

#[cfg(feature = "openai")]
#[tokio::test]
async fn openai_adapter_reports_api_errors() {
    use sandal::plug::openai::OpenAiModel;
    let (base, _) = http::serve_once(401, r#"{"error":{"message":"bad key"}}"#).await;
    let err = OpenAiModel::new("m", &base, None, "max_tokens").complete("s", "p", 1).await.unwrap_err();
    assert!(err.to_string().contains("bad key") && err.to_string().contains("401"), "{err}");
    let (base, _) = http::serve_once(200, r#"{"choices":[]}"#).await;
    assert!(OpenAiModel::new("m", &base, None, "max_tokens").complete("s", "p", 1).await.is_err());
}

#[cfg(feature = "anthropic")]
#[tokio::test]
async fn anthropic_adapter_speaks_the_messages_api() {
    use sandal::plug::anthropic::AnthropicModel;
    let reply = r#"{"content":[{"type":"text","text":"hello "},{"type":"tool_use","name":"x"},{"type":"text","text":"back"}]}"#;
    let (base, seen) = http::serve_once(200, reply).await;
    let m = AnthropicModel::new("some-model", &base, Some("key-test".into()));
    assert_eq!(m.complete("be brief", "say hi", 55).await.unwrap(), "hello back");
    let req = seen.lock().unwrap().clone();
    assert!(req.starts_with("POST /v1/messages "), "{req}");
    let lower = req.to_lowercase();
    assert!(lower.contains("x-api-key: key-test") && lower.contains("anthropic-version: 2023-06-01"));
    let body = http::body_of(&req);
    assert_eq!(body["model"], "some-model");
    assert_eq!(body["max_tokens"], 55);
    assert_eq!(body["system"], "be brief");
    assert_eq!(body["messages"][0]["content"], "say hi");

    let (base, _) = http::serve_once(429, r#"{"error":{"message":"slow down"}}"#).await;
    let err = AnthropicModel::new("m", &base, None).complete("s", "p", 1).await.unwrap_err();
    assert!(err.to_string().contains("slow down"), "{err}");
}
