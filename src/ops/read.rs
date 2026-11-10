use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{bullets, describe, non_empty, render, ForwardIn, Forward, SYSTEM};
use crate::ctx::Ctx;
use crate::error::Result;
use crate::fx::ModelFx;
use crate::id::{Id, ThingId};
use crate::op::Op;
use crate::thing::{Grade, Map, Payload, Reading, TrialResult};

/// Says what a thing is useful for, given the map and the evidence so far.
pub struct Read;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReadIn {
    pub thing: ThingId,
    pub map: Id<Map>,
}

#[derive(Deserialize)]
struct RawReading {
    text: String,
    useful_for: Vec<String>,
    caveats: Vec<String>,
}

const TEMPLATE: &str = include_str!("../prompts/read.txt");

#[async_trait]
impl Op for Read {
    type Fx = ModelFx;
    type In = ReadIn;
    type Out = Id<Reading>;
    const NAME: &'static str = "read";
    const VERSION: u32 = 1;

    async fn run(&self, ctx: &Ctx<ModelFx>, i: ReadIn) -> Result<Id<Reading>> {
        let thing = ctx.thing(i.thing)?;
        let map = ctx.get(i.map)?;

        // Runs through ctx.run, so the dependency on descendants is recorded.
        let descendants = ctx.run(&Forward, ForwardIn { thing: i.thing, depth: 3 }).await?;

        let mut results = Vec::new();
        let mut uncertain = Vec::new();
        for d in descendants.iter().filter(|d| d.kind == TrialResult::KIND) {
            let r: TrialResult = ctx.get(Id::<TrialResult>::from_raw(d.id))?;
            let proposal = ctx.get(r.proposal)?;
            let line = format!(
                "- Trial of: {}\n  Trial: {}\n  Grade: {}\n  Observed: {}",
                proposal.rebuilt, proposal.trial.description, r.grade, r.observed
            );
            if r.grade == Grade::Uncertain {
                uncertain.push(line);
            } else {
                results.push(line);
            }
        }
        let none = |v: Vec<String>| if v.is_empty() { "(none)".to_string() } else { v.join("\n") };

        let thing_text = match describe(&thing) {
            Ok(d) => d.text(true),
            Err(_) => serde_json::to_string_pretty(&thing.payload)?,
        };
        let parts = map
            .parts
            .iter()
            .map(|p| format!("Part: {}\n{}\nConditions:\n{}", p.name, p.description, bullets(&p.conditions)))
            .collect::<Vec<_>>()
            .join("\n\n");
        let prompt = render(
            TEMPLATE,
            &[
                ("thing", &thing_text),
                ("parts", &parts),
                ("results", &none(results)),
                ("uncertain", &none(uncertain)),
            ],
        );
        let raw: RawReading = ctx.ask_json(SYSTEM, &prompt).await?;
        non_empty(&raw.text, "text")?;
        ctx.put(
            Reading {
                of: i.thing,
                map: i.map,
                text: raw.text,
                useful_for: raw.useful_for,
                caveats: raw.caveats,
            },
            &[i.thing, i.map.raw()],
        )
    }
}
