//! One CLI smoke test: init, add, log in a temporary directory.

use std::process::Command;

fn sandal(dir: &std::path::Path, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_sandal"))
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn init_add_log() {
    let dir = tempfile::tempdir().unwrap();

    // Nothing works before init.
    let (ok, _, err) = sandal(dir.path(), &["log"]);
    assert!(!ok);
    assert!(err.contains("sandal init"), "{err}");

    let (ok, out, err) = sandal(dir.path(), &["init"]);
    assert!(ok, "{err}");
    assert!(out.contains("initialised"));
    for f in ["ledger.jsonl", "memo.jsonl", "config.toml"] {
        assert!(dir.path().join(".sandal").join(f).exists(), "{f}");
    }
    // Init again keeps the config.
    assert!(sandal(dir.path(), &["init"]).1.contains("kept the existing"));

    let (ok, source_id, err) = sandal(
        dir.path(),
        &["add", "source", "--title", "Burrs on a dog", "--body", "Hooks catch fur", "--domain", "botany", "--outcome", "They cling"],
    );
    assert!(ok, "{err}");
    let source_id = source_id.trim().to_string();
    assert_eq!(source_id.len(), 64);
    let (ok, idea_id, err) = sandal(dir.path(), &["add", "idea", "--title", "A fastener", "--body", "Something that clings"]);
    assert!(ok, "{err}");

    let (ok, log, err) = sandal(dir.path(), &["log"]);
    assert!(ok, "{err}");
    assert!(log.contains("Burrs on a dog") && log.contains("A fastener"), "{log}");
    assert!(log.lines().next().unwrap().contains("idea"), "newest first: {log}");

    let (_, only_sources, _) = sandal(dir.path(), &["log", "--kind", "source_item"]);
    assert!(only_sources.contains("Burrs on a dog") && !only_sources.contains("A fastener"));

    // Ids may be given as a unique prefix of at least 8 hex characters.
    let (ok, shown, err) = sandal(dir.path(), &["show", &source_id[..8]]);
    assert!(ok, "{err}");
    assert!(shown.contains("source_item") && shown.contains("user"), "{shown}");
    assert!(!sandal(dir.path(), &["show", &source_id[..4]]).0);
    let _ = idea_id;

    // Commands that never call the model work without a key or a model name.
    let (ok, forward, err) = sandal(dir.path(), &["forward", &source_id[..8]]);
    assert!(ok, "{err}");
    assert!(forward.contains("nothing yet"));
    let (ok, stats, _) = sandal(dir.path(), &["stats"]);
    assert!(ok && stats.contains("ledger: 2 things"), "{stats}");
    let (ok, stale, _) = sandal(dir.path(), &["stale"]);
    assert!(ok && stale.contains("nothing is stale"));

    // A command that needs the model says what is missing instead of failing obscurely.
    let (ok, _, err) = sandal(dir.path(), &["back", &source_id[..8]]);
    assert!(!ok);
    assert!(err.contains("name is required") || err.contains("anthropic"), "{err}");
}
