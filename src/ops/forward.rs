use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::ctx::Ctx;
use crate::error::Result;
use crate::fx::PureFx;
use crate::id::{Id, ThingId};
use crate::op::Op;
use crate::thing::{Descendant, Payload, TrialResult};

/// Reports what a thing led to.
pub struct Forward;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ForwardIn {
    pub thing: ThingId,
    pub depth: u8,
}

#[async_trait]
impl Op for Forward {
    type Fx = PureFx;
    type In = ForwardIn;
    type Out = Vec<Descendant>;
    const NAME: &'static str = "forward";
    const VERSION: u32 = 1;

    /// Breadth-first over `ctx.children`, to `depth` levels. Sorted by
    /// (depth, id), so deterministic. Appends nothing.
    async fn run(&self, ctx: &Ctx<PureFx>, i: ForwardIn) -> Result<Vec<Descendant>> {
        let mut depth_of: BTreeMap<ThingId, u8> = BTreeMap::new();
        let mut frontier = vec![i.thing];
        for d in 1..=i.depth {
            let mut next = Vec::new();
            for node in frontier {
                for child in ctx.children(node) {
                    if child != i.thing && !depth_of.contains_key(&child) {
                        depth_of.insert(child, d);
                        next.push(child);
                    }
                }
            }
            frontier = next;
        }
        let mut out = Vec::with_capacity(depth_of.len());
        for (id, depth) in depth_of {
            let thing = ctx.thing(id)?;
            let grade = if thing.kind == TrialResult::KIND {
                Some(ctx.get(Id::<TrialResult>::from_raw(id))?.grade)
            } else {
                None
            };
            out.push(Descendant {
                id,
                kind: thing.kind,
                depth,
                grade,
            });
        }
        out.sort_by_key(|d| (d.depth, d.id));
        Ok(out)
    }
}
