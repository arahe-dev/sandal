//! CLI tests: init, add, log in a temporary directory; and the agent protocol
//! (`--json`, exit codes, the handoff provider) end to end.

use serde_json::Value;
use std::process::Command;

struct Run {
    code: i32,
    out: String,
    err: String,
}

fn sandal(dir: &std::path::Path, args: &[&str]) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_sandal"))
        .current_dir(dir)
        .env_remove("SANDAL_PROVIDER")
        .env_remove("SANDAL_COMMAND")
        .args(args)
        .output()
        .unwrap();
    Run {
        code: out.status.code().unwrap(),
        out: String::from_utf8_lossy(&out.stdout).into_owned(),
        err: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn sandal_stdin(dir: &std::path::Path, args: &[&str], input: &str) -> Run {
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_sandal"))
        .current_dir(dir)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    Run {
        code: out.status.code().unwrap(),
        out: String::from_utf8_lossy(&out.stdout).into_owned(),
        err: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn json(r: &Run) -> Value {
    serde_json::from_str(&r.out).unwrap_or_else(|e| panic!("not JSON ({e}): {}\nstderr: {}", r.out, r.err))
}

#[test]
fn init_add_log() {
    let dir = tempfile::tempdir().unwrap();

    // Nothing works before init.
    let r = sandal(dir.path(), &["log"]);
    assert_ne!(r.code, 0);
    assert!(r.err.contains("sandal init"), "{}", r.err);

    let r = sandal(dir.path(), &["init"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.contains("initialised"));
    for f in ["ledger.jsonl", "memo.jsonl", "config.toml"] {
        assert!(dir.path().join(".sandal").join(f).exists(), "{f}");
    }
    // Init again keeps the config.
    assert!(sandal(dir.path(), &["init"]).out.contains("kept the existing"));

    let r = sandal(
        dir.path(),
        &["add", "source", "--title", "Burrs on a dog", "--body", "Hooks catch fur", "--domain", "botany", "--outcome", "They cling"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    let source_id = r.out.trim().to_string();
    assert_eq!(source_id.len(), 64);
    let r = sandal(dir.path(), &["add", "idea", "--title", "A fastener", "--body", "Something that clings"]);
    assert_eq!(r.code, 0, "{}", r.err);

    let r = sandal(dir.path(), &["log"]);
    assert!(r.out.contains("Burrs on a dog") && r.out.contains("A fastener"), "{}", r.out);
    assert!(r.out.lines().next().unwrap().contains("idea"), "newest first: {}", r.out);
    let only = sandal(dir.path(), &["log", "--kind", "source_item"]);
    assert!(only.out.contains("Burrs on a dog") && !only.out.contains("A fastener"));

    // Ids may be given as a unique prefix of at least 8 hex characters.
    let r = sandal(dir.path(), &["show", &source_id[..8]]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.contains("source_item") && r.out.contains("user"), "{}", r.out);
    assert_ne!(sandal(dir.path(), &["show", &source_id[..4]]).code, 0);

    // Commands that never call the model work with any provider.
    let r = sandal(dir.path(), &["forward", &source_id[..8]]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.contains("nothing yet"));
    assert!(sandal(dir.path(), &["stats"]).out.contains("ledger: 2 things"));
    assert!(sandal(dir.path(), &["stale"]).out.contains("nothing is stale"));
    assert!(sandal(dir.path(), &["pending"]).out.contains("nothing is waiting"));
}

#[test]
fn json_output_and_json_errors() {
    let dir = tempfile::tempdir().unwrap();
    let r = sandal(dir.path(), &["--json", "log"]);
    assert_eq!(r.code, 1);
    let j = json(&r);
    assert_eq!((j["ok"].as_bool(), j["status"].as_str()), (Some(false), Some("error")));
    assert!(j["error"].as_str().unwrap().contains("sandal init"));

    assert_eq!(json(&sandal(dir.path(), &["--json", "init"]))["config_written"], true);
    let r = sandal(dir.path(), &["--json", "add", "idea", "--title", "t", "--body", "b"]);
    let id = json(&r)["id"].as_str().unwrap().to_string();
    let j = json(&sandal(dir.path(), &["--json", "show", &id[..10]]));
    assert_eq!(j["ok"], true);
    assert_eq!(j["thing"]["kind"], "idea");
    let j = json(&sandal(dir.path(), &["--json", "log"]));
    assert_eq!(j["things"][0]["id"], id.as_str());
    let j = json(&sandal(dir.path(), &["--json", "stats"]));
    assert_eq!(j["things"], 1);
}

#[test]
fn handoff_round_trip_through_the_cli() {
    let dir = tempfile::tempdir().unwrap();
    sandal(dir.path(), &["init"]); // the default provider is handoff
    let id = sandal(dir.path(), &["add", "idea", "--title", "Hooks that catch loops", "--body", "Burrs cling to fur"]).out.trim().to_string();

    // 1. SANDAL needs a model reply. It says so, exits 3 and names the files.
    let r = sandal(dir.path(), &["--json", "back", &id[..8]]);
    assert_eq!(r.code, 3, "{}{}", r.out, r.err);
    let j = json(&r);
    assert_eq!((j["ok"].as_bool(), j["status"].as_str()), (Some(false), Some("pending")));
    let pending = j["pending"].as_array().unwrap();
    assert_eq!(pending.len(), 1);
    let prompt = std::fs::read_to_string(pending[0]["prompt_file"].as_str().unwrap()).unwrap();
    assert!(prompt.contains("Hooks that catch loops"));
    let listed = json(&sandal(dir.path(), &["--json", "pending"]));
    assert_eq!(listed["pending"].as_array().unwrap().len(), 1);

    // The text form says the same on stderr, still exit 3.
    let r = sandal(dir.path(), &["back", &id[..8]]);
    assert_eq!(r.code, 3);
    assert!(r.err.contains("waiting on 1 model reply"), "{}", r.err);

    // 2. The harness replies with `sandal answer` (reply on stdin) and runs the same command again.
    assert!(pending[0]["prompt"].as_str().unwrap().contains("TASK: back"), "the prompt text is in the listing");
    let key = pending[0]["key"].as_str().unwrap();
    let r = sandal_stdin(
        dir.path(),
        &["--json", "answer", &key[..8]],
        r#"{"principles":["tiny hooks catch loops"],"conditions":["a loop surface exists"]}"#,
    );
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert_eq!(json(&r)["remaining"], 0);
    let r = sandal(dir.path(), &["--json", "back", &id[..8]]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    let j = json(&r);
    assert_eq!(j["ok"], true);
    assert_eq!(j["principles"][0], "tiny hooks catch loops");
    assert_eq!(j["stats"]["model_calls"], 1);
    assert_eq!(json(&sandal(dir.path(), &["--json", "pending"]))["pending"].as_array().unwrap().len(), 0);

    // 3. Asked again, it is a memo hit: no model call, so nothing pending.
    let j = json(&sandal(dir.path(), &["--json", "back", &id[..8]]));
    assert_eq!(j["stats"]["model_calls"], 0);
}

#[test]
fn a_command_provider_without_a_command_says_what_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    sandal(dir.path(), &["init"]);
    let id = sandal(dir.path(), &["add", "idea", "--title", "t", "--body", "b"]).out.trim().to_string();
    let cfg = dir.path().join(".sandal/config.toml");
    let text = std::fs::read_to_string(&cfg).unwrap().replace("provider = \"handoff\"", "provider = \"command\"");
    std::fs::write(&cfg, text).unwrap();
    let r = sandal(dir.path(), &["back", &id[..8]]);
    assert_eq!(r.code, 1);
    assert!(r.err.contains("command is required"), "{}", r.err);
    // The environment can switch it back without editing the file.
    let out = Command::new(env!("CARGO_BIN_EXE_sandal"))
        .current_dir(dir.path())
        .env("SANDAL_PROVIDER", "handoff")
        .args(["back", &id[..8]])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
}
