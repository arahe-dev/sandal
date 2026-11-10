use std::collections::BTreeMap;
use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::id::{Id, ThingId};

/// What a stored payload type must provide: a kind tag.
pub trait Payload: Serialize + DeserializeOwned + Clone + Send + Sync + 'static {
    const KIND: &'static str;
}

macro_rules! payload {
    ($t:ty, $kind:literal) => {
        impl Payload for $t {
            const KIND: &'static str = $kind;
        }
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    Pure,
    Model,
    World,
}

impl fmt::Display for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Effect::Pure => "pure",
            Effect::Model => "model",
            Effect::World => "world",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MadeBy {
    User,
    Source(String),   // source name
    Verifier(String), // verifier name
    Op {
        name: String,
        version: u32,
        effect: Effect,
    },
}

impl fmt::Display for MadeBy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MadeBy::User => f.write_str("user"),
            MadeBy::Source(s) => write!(f, "source:{s}"),
            MadeBy::Verifier(s) => write!(f, "verifier:{s}"),
            MadeBy::Op { name, version, .. } => write!(f, "op:{name}@{version}"),
        }
    }
}

/// An immutable unit in the ledger.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Thing {
    pub id: ThingId,
    pub kind: String, // Payload::KIND
    pub payload: serde_json::Value,
    pub parents: Vec<ThingId>, // sorted, deduplicated
    pub made_by: MadeBy,
    pub at: u64, // unix millis, informational only
}

impl Thing {
    /// Decode the payload as `T`, checking the kind first.
    pub fn decode<T: Payload>(&self) -> crate::Result<T> {
        if self.kind != T::KIND {
            return Err(crate::Error::WrongKind {
                expected: T::KIND.to_string(),
                found: self.kind.clone(),
            });
        }
        Ok(serde_json::from_value(self.payload.clone())?)
    }
}

/// Something that was tried elsewhere. `outcome` is kept separate so that
/// early operations can be shown the item without it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceItem {
    pub title: String,
    pub body: String,
    pub domain: String,
    pub outcome: Option<String>,
    pub url: Option<String>,
}
payload!(SourceItem, "source_item");

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Map {
    pub name: String,
    pub summary: String,
    pub parts: Vec<Part>,
}
payload!(Map, "map");

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub name: String,
    pub description: String,
    pub conditions: Vec<String>,
}

/// Output of `back`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Origin {
    pub of: ThingId,
    pub principles: Vec<String>,
    pub conditions: Vec<String>,
}
payload!(Origin, "origin");

/// Output of `match`. No score is stored; ordering is derived when needed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Match {
    pub origin: Id<Origin>,
    pub map: Id<Map>,
    pub part: String,
    pub shared: Vec<String>,  // conditions present on both sides
    pub missing: Vec<String>, // conditions the source had and the target lacks
    pub note: String,
}
payload!(Match, "match");

/// Output of `carry`. A claim.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    pub source: ThingId,
    pub origin: Id<Origin>,
    pub map: Id<Map>,
    pub part: String,
    pub rebuilt: String, // the source idea rebuilt for this part
    pub root: String,    // the assumption this proposal rests on, one phrase
    pub trial: TrialSpec,
    pub if_right: String,
    pub if_wrong: String,
}
payload!(Proposal, "proposal");

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrialSpec {
    pub kind: String, // selects a verifier template
    pub params: BTreeMap<String, String>,
    pub description: String,
}

/// Entered by a verifier. Parent = the proposal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrialResult {
    pub proposal: Id<Proposal>,
    pub grade: Grade,
    pub observed: String,
    pub metrics: BTreeMap<String, f64>,
}
payload!(TrialResult, "trial_result");

/// The grade describes the evidence, never the idea.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Grade {
    Confirmed,
    Partial,
    Refuted,
    Uncertain,
}

impl fmt::Display for Grade {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Grade::Confirmed => "confirmed",
            Grade::Partial => "partial",
            Grade::Refuted => "refuted",
            Grade::Uncertain => "uncertain",
        })
    }
}

/// Output of `roots`: proposals with their root phrases canonicalised.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RootLabels {
    pub labels: Vec<(Id<Proposal>, String)>,
}
payload!(RootLabels, "root_labels");

/// Output of `mix`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Study {
    pub groups: Vec<Group>,     // sorted by label
    pub order: Vec<Id<Proposal>>, // what to try, in order
}
payload!(Study, "study");

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub label: String,
    pub members: Vec<Id<Proposal>>,
}

/// Output of `pair`: proposals from different roots, to be tried together.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pairing {
    pub pairs: Vec<(Id<Proposal>, Id<Proposal>)>,
}
payload!(Pairing, "pairing");

/// Output of `read`. Free text, no verdict field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    pub of: ThingId,
    pub map: Id<Map>,
    pub text: String,
    pub useful_for: Vec<String>,
    pub caveats: Vec<String>,
}
payload!(Reading, "reading");

/// A generic user-entered thing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Idea {
    pub title: String,
    pub body: String,
}
payload!(Idea, "idea");

/// Output of `forward`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Descendant {
    pub id: ThingId,
    pub kind: String,
    pub depth: u8,
    pub grade: Option<Grade>,
}
