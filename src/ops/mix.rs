use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::ctx::Ctx;
use crate::error::Result;
use crate::fx::PureFx;
use crate::id::Id;
use crate::op::Op;
use crate::thing::{Group, Match, Payload, Proposal, RootLabels, Study};

/// Decides what to try and in which order. No model call.
pub struct Mix;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MixIn {
    pub labels: Id<RootLabels>,
    pub max_trials: usize,
}

/// How many conditions the proposal's match says the source needed and the
/// target lacks.
fn missing_count(ctx: &Ctx<PureFx>, proposal: Id<Proposal>) -> Result<usize> {
    let thing = ctx.thing(proposal.raw())?;
    for parent in thing.parents {
        if ctx.thing(parent)?.kind == Match::KIND {
            return Ok(ctx.get(Id::<Match>::from_raw(parent))?.missing.len());
        }
    }
    // DECISION: a proposal with no match parent (one entered by hand) counts
    // as missing nothing.
    Ok(0)
}

#[async_trait]
impl Op for Mix {
    type Fx = PureFx;
    type In = MixIn;
    type Out = Id<Study>;
    const NAME: &'static str = "mix";
    const VERSION: u32 = 1;

    async fn run(&self, ctx: &Ctx<PureFx>, i: MixIn) -> Result<Id<Study>> {
        let labels = ctx.get(i.labels)?;

        // Group proposals by label. Within a group, fewest missing conditions
        // first, ties by id.
        let mut by_label: BTreeMap<String, Vec<(usize, Id<Proposal>)>> = BTreeMap::new();
        for (proposal, label) in labels.labels {
            let missing = missing_count(ctx, proposal)?;
            by_label.entry(label).or_default().push((missing, proposal));
        }
        let groups: Vec<Group> = by_label
            .into_iter()
            .map(|(label, mut members)| {
                members.sort();
                Group {
                    label,
                    members: members.into_iter().map(|(_, p)| p).collect(),
                }
            })
            .collect(); // BTreeMap order: sorted by label

        // Order groups by size, largest first, ties by label. The largest group
        // goes first because one trial there informs the most proposals.
        let mut ranked: Vec<&Group> = groups.iter().collect();
        ranked.sort_by(|a, b| b.members.len().cmp(&a.members.len()).then(a.label.cmp(&b.label)));

        // DECISION: "the remaining members in the same group order" is read as:
        // first the head of every group, then the rest of each group in turn.
        let mut order: Vec<Id<Proposal>> = ranked.iter().map(|g| g.members[0]).collect();
        for g in &ranked {
            order.extend(g.members.iter().skip(1).copied());
        }
        order.truncate(i.max_trials);

        ctx.put(Study { groups, order }, &[i.labels.raw()])
    }
}
