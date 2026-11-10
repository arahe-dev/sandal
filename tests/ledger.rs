use sandal::ledger::{now_millis, Ledger, Slot};
use sandal::{Idea, MadeBy};
use std::fs;

fn idea(n: &str) -> Idea {
    Idea { title: n.into(), body: format!("body {n}") }
}

fn lines(dir: &std::path::Path) -> usize {
    fs::read_to_string(dir.join("ledger.jsonl")).unwrap().lines().filter(|l| !l.is_empty()).count()
}

#[test]
fn append_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let l = Ledger::open(dir.path()).unwrap();
    let (a, first) = l.put(&idea("a"), &[], MadeBy::User).unwrap();
    let (child, _) = l.put(&idea("c"), &[a.raw()], MadeBy::User).unwrap();
    assert!(first.new);
    let before = l.version(&Slot::Children(a.raw()));
    let n = lines(dir.path());
    let (child2, again) = l.put(&idea("c"), &[a.raw()], MadeBy::User).unwrap();
    assert_eq!(child, child2);
    assert!(!again.new);
    assert!(again.bumped.is_empty());
    assert_eq!(lines(dir.path()), n);
    assert_eq!(l.version(&Slot::Children(a.raw())), before);
}

#[test]
fn children_index() {
    let dir = tempfile::tempdir().unwrap();
    let l = Ledger::open(dir.path()).unwrap();
    let (a, _) = l.put(&idea("a"), &[], MadeBy::User).unwrap();
    assert_eq!(l.version(&Slot::Children(a.raw())), 0);
    let (c, appended) = l.put(&idea("c"), &[a.raw()], MadeBy::User).unwrap();
    assert_eq!(appended.bumped, vec![Slot::Children(a.raw())]);
    assert_eq!(l.children(&a.raw()), vec![c.raw()]);
    assert_eq!(l.version(&Slot::Children(a.raw())), 1);
}

#[test]
fn head_last_wins() {
    let dir = tempfile::tempdir().unwrap();
    let l = Ledger::open(dir.path()).unwrap();
    let (a, _) = l.put(&idea("a"), &[], MadeBy::User).unwrap();
    let (b, _) = l.put(&idea("b"), &[], MadeBy::User).unwrap();
    assert_eq!(l.set_head("h", a.raw()).unwrap(), 1);
    assert_eq!(l.set_head("h", b.raw()).unwrap(), 2);
    assert_eq!(l.head("h"), Some(b.raw()));
    assert_eq!(l.version(&Slot::Head("h".into())), 2);
}

#[test]
fn reload_reproduces_state() {
    let dir = tempfile::tempdir().unwrap();
    let (things, children, heads, versions);
    {
        let l = Ledger::open(dir.path()).unwrap();
        let (a, _) = l.put(&idea("a"), &[], MadeBy::User).unwrap();
        let (b, _) = l.put(&idea("b"), &[a.raw()], MadeBy::User).unwrap();
        l.put(&idea("c"), &[a.raw(), b.raw()], MadeBy::Source("s".into())).unwrap();
        l.set_head("h", b.raw()).unwrap();
        l.set_head("h", a.raw()).unwrap();
        things = l.things();
        children = l.children(&a.raw());
        heads = l.heads();
        versions = (
            l.version(&Slot::Children(a.raw())),
            l.version(&Slot::Children(b.raw())),
            l.version(&Slot::Head("h".into())),
        );
    }
    let l = Ledger::open(dir.path()).unwrap();
    assert_eq!(l.things(), things);
    let a = things[0].id;
    let b = things[1].id;
    assert_eq!(l.children(&a), children);
    assert_eq!(l.heads(), heads);
    assert_eq!(
        (
            l.version(&Slot::Children(a)),
            l.version(&Slot::Children(b)),
            l.version(&Slot::Head("h".into()))
        ),
        versions
    );
}

#[test]
fn corrupt_last_line_tolerated() {
    let dir = tempfile::tempdir().unwrap();
    {
        let l = Ledger::open(dir.path()).unwrap();
        l.put(&idea("a"), &[], MadeBy::User).unwrap();
        l.put(&idea("b"), &[], MadeBy::User).unwrap();
    }
    let path = dir.path().join("ledger.jsonl");
    // Truncate the final line mid-record.
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, &text[..text.len() - 15]).unwrap();
    let l = Ledger::open(dir.path()).unwrap();
    assert_eq!(l.len(), 1);
    // Later appends start on a clean line and the file stays readable.
    l.put(&idea("c"), &[], MadeBy::User).unwrap();
    drop(l);
    assert_eq!(Ledger::open(dir.path()).unwrap().len(), 2);

    // A bad line anywhere else is an error.
    let text = fs::read_to_string(&path).unwrap();
    let mut ls: Vec<&str> = text.lines().collect();
    ls.insert(0, "{ not json");
    fs::write(&path, ls.join("\n") + "\n").unwrap();
    assert!(Ledger::open(dir.path()).is_err());
}

#[test]
fn missing_parent_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let l = Ledger::open(dir.path()).unwrap();
    let ghost = sandal::ThingId([7; 32]);
    assert!(l.put(&idea("a"), &[ghost], MadeBy::User).is_err());
    assert!(l.set_head("h", ghost).is_err());
    assert!(now_millis() > 0);
}

#[test]
fn prefix_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let l = Ledger::open(dir.path()).unwrap();
    let (a, _) = l.put(&idea("a"), &[], MadeBy::User).unwrap();
    let hex = a.raw().to_string();
    assert_eq!(l.resolve_prefix(&hex[..8]).unwrap(), a.raw());
    assert!(l.resolve_prefix(&hex[..6]).is_err());
    assert!(l.resolve_prefix("00000000").is_err());
}
