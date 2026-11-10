use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{describe, render, SYSTEM};
use crate::ctx::Ctx;
use crate::error::{Error, Result};
use crate::fx::ModelFx;
use crate::id::{Id, ThingId};
use crate::op::Op;
use crate::thing::Origin;

/// Extracts what made a thing work. Accepts a source item, an idea or a proposal.
pub struct Back;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackIn {
    pub thing: ThingId,
}

#[derive(Deserialize)]
struct RawOrigin {
    principles: Vec<String>,
    conditions: Vec<String>,
}

const TEMPLATE: &str = include_str!("../prompts/back.txt");

#[async_trait]
impl Op for Back {
    type Fx = ModelFx;
    type In = BackIn;
    type Out = Id<Origin>;
    const NAME: &'static str = "back";
    const VERSION: u32 = 1;

    async fn run(&self, ctx: &Ctx<ModelFx>, i: BackIn) -> Result<Id<Origin>> {
        let thing = ctx.thing(i.thing)?;
        let d = describe(&thing)?;
        // The prompt shows title, body and domain. It never shows the outcome.
        let prompt = render(
            TEMPLATE,
            &[("title", &d.title), ("domain", &d.domain), ("body", &d.body)],
        );
        let raw: RawOrigin = ctx.ask_json(SYSTEM, &prompt).await?;
        for (what, list) in [("principles", &raw.principles), ("conditions", &raw.conditions)] {
            if !(1..=6).contains(&list.len()) || list.iter().any(|s| s.trim().is_empty()) {
                return Err(Error::BadModelOutput(format!(
                    "{what}: expected 1 to 6 non-empty entries, got {}",
                    list.len()
                )));
            }
        }
        ctx.put(
            Origin {
                of: i.thing,
                principles: raw.principles,
                conditions: raw.conditions,
            },
            &[i.thing],
        )
    }
}
