use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{non_empty, render, SYSTEM};
use crate::ctx::Ctx;
use crate::error::{Error, Result};
use crate::fx::ModelFx;
use crate::id::Id;
use crate::op::Op;
use crate::thing::{Proposal, RootLabels};

/// Gives proposals that rest on the same assumption the same label.
pub struct Roots;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RootsIn {
    pub proposals: Vec<Id<Proposal>>,
}

#[derive(Deserialize)]
struct RawLabels {
    labels: Vec<String>,
}

const TEMPLATE: &str = include_str!("../prompts/roots.txt");

#[async_trait]
impl Op for Roots {
    type Fx = ModelFx;
    type In = RootsIn;
    type Out = Id<RootLabels>;
    const NAME: &'static str = "roots";
    const VERSION: u32 = 1;

    /// Input ids are sorted before the memo key is computed, so order does not matter.
    fn normalize(input: &mut RootsIn) {
        input.proposals.sort();
        input.proposals.dedup();
    }

    async fn run(&self, ctx: &Ctx<ModelFx>, i: RootsIn) -> Result<Id<RootLabels>> {
        if i.proposals.is_empty() {
            return ctx.put(RootLabels { labels: Vec::new() }, &[]);
        }
        let mut lines = Vec::with_capacity(i.proposals.len());
        for (n, id) in i.proposals.iter().enumerate() {
            let p = ctx.get(*id)?;
            lines.push(format!("{}. {}", n + 1, p.root));
        }
        let prompt = render(TEMPLATE, &[("proposals", &lines.join("\n"))]);
        let raw: RawLabels = ctx.ask_json(SYSTEM, &prompt).await?;
        if raw.labels.len() != i.proposals.len() {
            return Err(Error::BadModelOutput(format!(
                "expected {} labels, got {}",
                i.proposals.len(),
                raw.labels.len()
            )));
        }
        for l in &raw.labels {
            non_empty(l, "label")?;
        }
        let parents: Vec<_> = i.proposals.iter().map(|p| p.raw()).collect();
        let labels = i
            .proposals
            .iter()
            .copied()
            .zip(raw.labels.into_iter().map(|l| l.trim().to_string()))
            .collect();
        ctx.put(RootLabels { labels }, &parents)
    }
}
