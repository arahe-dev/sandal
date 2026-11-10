//! SANDAL: carries ideas that worked in one place into a new problem, proposes
//! trials for them, and re-reads its own conclusions when new evidence arrives.

pub mod bench;
pub mod config;
pub mod ctx;
pub mod engine;
pub mod error;
pub mod fx;
pub mod id;
pub mod ledger;
pub mod memo;
pub mod op;
pub mod ops;
pub mod plug;
pub mod thing;
pub mod verify;

pub use ctx::Ctx;
pub use engine::{Budget, Engine, Plugs, Stats};
pub use error::{Error, Result};
pub use fx::{Allows, Fx, ModelFx, PureFx, WorldFx};
pub use id::{Id, ThingId};
pub use ledger::{Ledger, Slot};
pub use memo::MemoEntry;
pub use op::{each, iterate, keep_ok, then, Op};
pub use ops::{
    Back, BackIn, Carry, CarryIn, Cycle, CycleIn, CycleOut, Forward, ForwardIn, MapIn, MapOp, MatchIn, MatchOp, Mix,
    MixIn, Notice, NoticeIn, Read, ReadIn, Roots, RootsIn, Try, TryIn,
};
pub use plug::{Model, Source, SourceQuery, TrialKind, TrialOutcome, Verifier};
pub use thing::*;
pub use verify::Verified;
