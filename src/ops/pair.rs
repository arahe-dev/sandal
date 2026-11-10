//! Cross-root pairing: proposals from complementary roots, tried together.
//!
//! `Mix` only orders proposals. A gadget often needs two things from different
//! roots at once (a weakness that is known, and a new channel it can travel
//! through), so `Pair` emits those pairs. It is an addition: `Mix` is unchanged.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::ctx::Ctx;
use crate::error::Result;
use crate::fx::PureFx;
use crate::id::Id;
use crate::op::Op;
use crate::thing::{Group, Pairing, Study};

/// Emits paired trials across complementary roots.
pub struct Pair;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairIn {
    pub study: Id<Study>,
    pub max_pairs: usize,
}

#[async_trait]
impl Op for Pair {
    type Fx = PureFx;
    type In = PairIn;
    type Out = Id<Pairing>;
    const NAME: &'static str = "pair";
    const VERSION: u32 = 1;

    /// Rank the groups the way `Mix` does (largest first, ties by label), take
    /// the best member of each, and pair them off in that order. Each pair
    /// joins two different roots, and no proposal appears twice.
    ///
    /// DECISION: "complementary" is read as "different root label". Nothing in
    /// the data says which roots are weaknesses and which are channels, so the
    /// pairing does not try to tell them apart; a study is paired by rank.
    async fn run(&self, ctx: &Ctx<PureFx>, i: PairIn) -> Result<Id<Pairing>> {
        let study = ctx.get(i.study)?;
        let mut ranked: Vec<&Group> = study.groups.iter().filter(|g| !g.members.is_empty()).collect();
        ranked.sort_by(|a, b| b.members.len().cmp(&a.members.len()).then(a.label.cmp(&b.label)));
        let mut heads = ranked.iter().map(|g| g.members[0]);
        let mut pairs = Vec::new();
        while pairs.len() < i.max_pairs {
            match (heads.next(), heads.next()) {
                (Some(a), Some(b)) => pairs.push((a, b)),
                _ => break,
            }
        }
        ctx.put(Pairing { pairs }, &[i.study.raw()])
    }
}
