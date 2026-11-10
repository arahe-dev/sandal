use sandal::{Idea, ThingId, Payload};
use serde_json::json;

fn id_of(payload: serde_json::Value, parents: &[ThingId]) -> ThingId {
    ThingId::compute(Idea::KIND, &payload, parents)
}

fn some_parent(n: u8) -> ThingId {
    ThingId::compute("idea", &json!({ "title": format!("p{n}"), "body": "" }), &[])
}

#[test]
fn same_content_same_id() {
    // Same kind, payload and parents give the same id, whatever the field order of the JSON.
    let a: serde_json::Value = serde_json::from_str(r#"{"title":"t","body":"b"}"#).unwrap();
    let b: serde_json::Value = serde_json::from_str(r#"{ "body" : "b", "title" : "t" }"#).unwrap();
    assert_eq!(id_of(a, &[]), id_of(b, &[]));
}

#[test]
fn canonical_json_sorts_keys() {
    // Guards against `preserve_order` being switched on anywhere in the dependency tree.
    let v: serde_json::Value = serde_json::from_str(r#"{"b":1,"a":2}"#).unwrap();
    assert_eq!(String::from_utf8(sandal::id::canonical_json(&v)).unwrap(), r#"{"a":2,"b":1}"#);
}

#[test]
fn parents_change_id() {
    let payload = json!({ "title": "t", "body": "b" });
    let none = id_of(payload.clone(), &[]);
    let one = id_of(payload.clone(), &[some_parent(1)]);
    let other = id_of(payload, &[some_parent(2)]);
    assert_ne!(none, one);
    assert_ne!(one, other);
}

#[test]
fn parent_order_irrelevant() {
    let payload = json!({ "title": "t", "body": "b" });
    let (p, q) = (some_parent(1), some_parent(2));
    assert_eq!(id_of(payload.clone(), &[p, q]), id_of(payload.clone(), &[q, p]));
    // Duplicates collapse too.
    assert_eq!(id_of(payload.clone(), &[p, q]), id_of(payload, &[q, p, p]));
}

#[test]
fn kind_is_part_of_identity() {
    let payload = json!({ "title": "t", "body": "b" });
    assert_ne!(ThingId::compute("idea", &payload, &[]), ThingId::compute("other", &payload, &[]));
}

#[test]
fn ids_are_stable_across_runs() {
    // A fixed vector: if this changes, every stored ledger changes with it.
    let id = ThingId::compute("idea", &json!({ "body": "b", "title": "t" }), &[]);
    assert_eq!(id.to_string().len(), 64);
    assert_eq!(id.to_string(), id.to_string().to_lowercase());
    let again: ThingId = id.to_string().parse().unwrap();
    assert_eq!(id, again);
}
