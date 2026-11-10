use std::fmt;
use std::marker::PhantomData;
use std::str::FromStr;

use serde::de::{self, Deserializer};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::thing::Payload;

/// sha256 over: kind tag, canonical JSON of the payload, sorted parent ids.
/// Canonical JSON = serde_json with sorted object keys, no whitespace.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ThingId(pub [u8; 32]); // Display and serde as lowercase hex

impl ThingId {
    /// Compute the id of a thing. Parents are sorted and deduplicated first, so
    /// the order they are given in does not matter.
    pub fn compute(kind: &str, payload: &Value, parents: &[ThingId]) -> ThingId {
        let parents = normalize_parents(parents);
        let payload = canonical_json(payload);
        let mut h = Sha256::new();
        h.update(kind.as_bytes());
        h.update([0u8]);
        h.update(&payload);
        h.update([0u8]);
        for p in &parents {
            h.update(p.0);
        }
        ThingId(h.finalize().into())
    }

    pub fn hex(&self) -> String {
        hex::encode(self.0)
    }

    /// The first eight hex characters, for display.
    pub fn short(&self) -> String {
        self.hex()[..8].to_string()
    }
}

impl fmt::Display for ThingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex())
    }
}

impl fmt::Debug for ThingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ThingId({})", self.short())
    }
}

impl FromStr for ThingId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = hex::decode(s).map_err(|e| format!("bad id {s:?}: {e}"))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| format!("bad id {s:?}: expected 64 hex characters"))?;
        Ok(ThingId(arr))
    }
}

impl Serialize for ThingId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.hex())
    }
}

impl<'de> Deserialize<'de> for ThingId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(de::Error::custom)
    }
}

/// Sorted, deduplicated copy of a parent list.
pub fn normalize_parents(parents: &[ThingId]) -> Vec<ThingId> {
    let mut v = parents.to_vec();
    v.sort();
    v.dedup();
    v
}

/// Canonical JSON: object keys sorted (serde_json is used without the
/// `preserve_order` feature, so its maps are ordered), no whitespace.
pub fn canonical_json(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).expect("a serde_json::Value always serialises")
}

/// sha256 over a list of byte parts, separated by a zero byte.
pub fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h.update([0u8]);
        }
        h.update(p);
    }
    h.finalize().into()
}

/// A ThingId that remembers its payload type. Zero runtime cost.
pub struct Id<T: Payload> {
    raw: ThingId,
    _t: PhantomData<fn() -> T>,
}

impl<T: Payload> Id<T> {
    /// Wrap a raw id. The kind is checked when the thing is loaded, not here.
    pub fn from_raw(raw: ThingId) -> Self {
        Id { raw, _t: PhantomData }
    }

    pub fn raw(&self) -> ThingId {
        self.raw
    }
}

impl<T: Payload> Clone for Id<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: Payload> Copy for Id<T> {}
impl<T: Payload> PartialEq for Id<T> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}
impl<T: Payload> Eq for Id<T> {}
impl<T: Payload> std::hash::Hash for Id<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.raw.hash(state)
    }
}
impl<T: Payload> PartialOrd for Id<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<T: Payload> Ord for Id<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.raw.cmp(&other.raw)
    }
}
impl<T: Payload> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id<{}>({})", T::KIND, self.raw.short())
    }
}
impl<T: Payload> fmt::Display for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.raw, f)
    }
}
impl<T: Payload> Serialize for Id<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.raw.serialize(s)
    }
}
impl<'de, T: Payload> Deserialize<'de> for Id<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Id::from_raw(ThingId::deserialize(d)?))
    }
}
