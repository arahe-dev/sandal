use sandal::plug::{CommandVerifier, FileSource, ParamTemplate, Template};
use sandal::*;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

fn item(title: &str, body: &str, domain: &str) -> SourceItem {
    SourceItem { title: title.into(), body: body.into(), domain: domain.into(), outcome: None, url: None }
}

fn write_items(path: &Path, items: &[SourceItem]) {
    let mut f = std::fs::File::create(path).unwrap();
    for i in items {
        writeln!(f, "{}", serde_json::to_string(i).unwrap()).unwrap();
    }
}

#[tokio::test]
async fn file_source_ranks_and_excludes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    write_items(
        &path,
        &[
            item("Zebra stripes", "Stripes confuse biting flies on hot grassland", "biology"),
            item("Fly screens", "Mesh keeps biting flies out of rooms", "housing"),
            item("Chess openings", "Control the centre early", "games"),
            item("Mosquito nets", "Nets keep biting flies and mosquitoes off sleepers", "health"),
        ],
    );
    let source = FileSource::new("notes", &path);
    let q = |exclude: Vec<&str>, k| sandal::SourceQuery {
        conditions: vec!["Biting flies reach the room through open windows".into()],
        exclude_domains: exclude.into_iter().map(String::from).collect(),
        k,
    };
    let hits = source.fetch(&q(vec![], 10)).await.unwrap();
    assert_eq!(hits.len(), 4);
    // "biting" and "flies" are shared by three items and nothing is shared with the chess
    // item. Ties on the count are broken by title.
    let titles: Vec<&str> = hits.iter().map(|h| h.title.as_str()).collect();
    assert_eq!(titles[3], "Chess openings", "{titles:?}");
    assert_eq!(&titles[..3], &["Fly screens", "Mosquito nets", "Zebra stripes"]);

    // Excluded domains are absent (whatever the case), and k cuts the list.
    let hits = source.fetch(&q(vec!["Housing"], 2)).await.unwrap();
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().all(|h| h.domain != "housing"));
}

#[tokio::test]
async fn file_source_reports_bad_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    std::fs::write(&path, "{ not json\n").unwrap();
    let q = sandal::SourceQuery { conditions: vec![], exclude_domains: vec![], k: 1 };
    assert!(FileSource::new("n", &path).fetch(&q).await.is_err());
    assert!(FileSource::new("n", dir.path().join("missing.jsonl")).fetch(&q).await.is_err());
}

// ---- command verifier -------------------------------------------------------
//
// The programs are fixed templates. `sh` on unix and `cmd` on windows stand in
// for "a program that exits with a given code"; they are never given a model's text.

fn shell(script: &str) -> (String, Vec<String>) {
    if cfg!(windows) {
        ("cmd".into(), vec!["/C".into(), script.into()])
    } else {
        ("sh".into(), vec!["-c".into(), script.into()])
    }
}

fn template(name: &str, script: &str, timeout_secs: u64, params: Vec<(&str, &str)>) -> Template {
    let (program, args) = shell(script);
    Template {
        name: name.into(),
        description: format!("test template {name}"),
        program,
        args,
        timeout_secs,
        params: params
            .into_iter()
            .map(|(n, r)| ParamTemplate { name: n.into(), regex: r.into() })
            .collect(),
    }
}

fn spec(kind: &str, params: &[(&str, &str)]) -> TrialSpec {
    TrialSpec {
        kind: kind.into(),
        params: params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<_, _>>(),
        description: "d".into(),
    }
}

#[tokio::test]
async fn command_verifier_maps_exit_codes() {
    let v = CommandVerifier::new(vec![
        template("zero", "exit 0", 20, vec![]),
        template("two", "exit 2", 20, vec![]),
        template("one", "exit 1", 20, vec![]),
        template("nine", "exit 9", 20, vec![]),
    ])
    .unwrap();
    assert_eq!(v.run(&spec("zero", &[])).await.unwrap().grade, Grade::Confirmed);
    assert_eq!(v.run(&spec("two", &[])).await.unwrap().grade, Grade::Partial);
    assert_eq!(v.run(&spec("one", &[])).await.unwrap().grade, Grade::Refuted);
    assert_eq!(v.run(&spec("nine", &[])).await.unwrap().grade, Grade::Refuted);
    // An unknown kind is an error, not a grade.
    assert!(v.run(&spec("nope", &[])).await.is_err());
    let kinds: Vec<String> = v.kinds().into_iter().map(|k| k.name).collect();
    assert_eq!(kinds, vec!["zero", "two", "one", "nine"]);
}

#[tokio::test]
async fn command_verifier_rejects_bad_param() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("started.txt");
    let script = if cfg!(windows) {
        format!("echo started>{}", marker.display())
    } else {
        format!("touch '{}'", marker.display())
    };
    let v = CommandVerifier::new(vec![template("t", &script, 20, vec![("NAME", "^[a-z]{1,8}$")])]).unwrap();
    // A parameter that fails its regex: Err, and the program is not started.
    assert!(v.run(&spec("t", &[("NAME", "no spaces; rm -rf /")])).await.is_err());
    assert!(v.run(&spec("t", &[])).await.is_err(), "a missing parameter is an error too");
    assert!(!marker.exists(), "the program must not have been started");
    // A good parameter runs it.
    assert_eq!(v.run(&spec("t", &[("NAME", "abc")])).await.unwrap().grade, Grade::Confirmed);
    assert!(marker.exists());
}

#[tokio::test]
async fn command_verifier_times_out() {
    let script = if cfg!(windows) { "ping -n 5 127.0.0.1 >nul" } else { "sleep 20" };
    let v = CommandVerifier::new(vec![template("slow", script, 1, vec![])]).unwrap();
    let start = std::time::Instant::now();
    let err = v.run(&spec("slow", &[])).await.unwrap_err();
    assert!(matches!(err, Error::Plug(_)));
    assert!(err.to_string().contains("timed out"), "{err}");
    assert!(start.elapsed() < std::time::Duration::from_secs(10));
}

#[tokio::test]
async fn command_verifier_parses_metrics() {
    let v = CommandVerifier::new(vec![template(
        "m",
        "echo metric mass_kg 1.25&& echo plain line&& echo metric bad notanumber",
        20,
        vec![],
    )])
    .unwrap();
    // `&&` chains work in both sh and cmd. (cmd keeps the space before `&&` in
    // echo output, which the parser tolerates.)
    let out = v.run(&spec("m", &[])).await.unwrap();
    assert_eq!(out.metrics.get("mass_kg"), Some(&1.25));
    assert_eq!(out.metrics.len(), 1);
    assert!(out.observed.contains("plain line"));
}

#[tokio::test]
async fn command_verifier_passes_params_as_environment_only() {
    let script = if cfg!(windows) { "echo metric got %SANDAL_PARAM_X%" } else { "echo metric got $SANDAL_PARAM_X" };
    let v = CommandVerifier::new(vec![template("env", script, 20, vec![("X", "^[0-9]+$")])]).unwrap();
    let out = v.run(&spec("env", &[("X", "42")])).await.unwrap();
    assert_eq!(out.metrics.get("got"), Some(&42.0));
}

#[test]
fn command_verifier_rejects_a_bad_regex_at_construction() {
    assert!(CommandVerifier::new(vec![template("t", "exit 0", 5, vec![("X", "([")])]).is_err());
}

#[tokio::test]
async fn command_verifier_keeps_only_the_last_2000_characters() {
    // 3000 characters of output through a fixed template.
    let script = if cfg!(windows) {
        "powershell -NoProfile -Command \"Write-Output ('a' * 3000)\""
    } else {
        "head -c 3000 /dev/zero | tr '\\0' 'a'"
    };
    let v = CommandVerifier::new(vec![template("long", script, 30, vec![])]).unwrap();
    let out = v.run(&spec("long", &[])).await.unwrap();
    assert!(out.observed.chars().count() <= 2000);
    assert!(out.observed.contains('a'));
}
