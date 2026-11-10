use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{bullets, render, SYSTEM};
use crate::ctx::Ctx;
use crate::error::Result;
use crate::fx::ModelFx;
use crate::id::Id;
use crate::op::Op;
use crate::thing::{Map, Match, Origin};

/// Compares a source's conditions with the target's, before anyone looks at how
/// the source turned out.
pub struct MatchOp;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatchIn {
    pub origin: Id<Origin>,
    pub map: Id<Map>,
}

#[derive(Deserialize)]
struct RawMatches {
    matches: Vec<RawMatch>,
}

#[derive(Deserialize)]
struct RawMatch {
    part: String,
    shared: Vec<String>,
    missing: Vec<String>,
    note: String,
}

const TEMPLATE: &str = include_str!("../prompts/match.txt");

#[async_trait]
impl Op for MatchOp {
    type Fx = ModelFx;
    type In = MatchIn;
    type Out = Vec<Id<Match>>;
    const NAME: &'static str = "match";
    const VERSION: u32 = 1;

    async fn run(&self, ctx: &Ctx<ModelFx>, i: MatchIn) -> Result<Vec<Id<Match>>> {
        let origin = ctx.get(i.origin)?;
        let map = ctx.get(i.map)?;
        // The prompt shows the Origin and the map's parts. It never shows the
        // source item, so it cannot see the outcome.
        let parts = map
            .parts
            .iter()
            .map(|p| {
                format!(
                    "Part: {}\nDescription: {}\nConditions:\n{}",
                    p.name,
                    p.description,
                    bullets(&p.conditions)
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        let prompt = render(
            TEMPLATE,
            &[
                ("principles", &bullets(&origin.principles)),
                ("conditions", &bullets(&origin.conditions)),
                ("parts", &parts),
            ],
        );
        let raw: RawMatches = ctx.ask_json(SYSTEM, &prompt).await?;

        let mut out = Vec::new();
        for m in raw.matches {
            // Entries whose part is not a part name in the map are dropped,
            // and so are entries with an empty `shared`.
            if !map.parts.iter().any(|p| p.name == m.part) || m.shared.is_empty() {
                continue;
            }
            out.push(ctx.put(
                Match {
                    origin: i.origin,
                    map: i.map,
                    part: m.part,
                    shared: m.shared,
                    missing: m.missing,
                    note: m.note,
                },
                &[i.origin.raw(), i.map.raw()],
            )?);
        }
        Ok(out)
    }
}
