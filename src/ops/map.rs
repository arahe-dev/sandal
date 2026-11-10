use std::collections::BTreeSet;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{non_empty, render, SYSTEM};
use crate::ctx::Ctx;
use crate::error::{Error, Result};
use crate::fx::ModelFx;
use crate::id::Id;
use crate::op::Op;
use crate::thing::{Map, Part};

/// Breaks a problem statement into parts.
pub struct MapOp;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MapIn {
    pub name: String,
    pub problem: String,
}

#[derive(Deserialize)]
struct RawMap {
    summary: String,
    parts: Vec<RawPart>,
}

#[derive(Deserialize)]
struct RawPart {
    name: String,
    description: String,
    conditions: Vec<String>,
}

const TEMPLATE: &str = include_str!("../prompts/map.txt");

#[async_trait]
impl Op for MapOp {
    type Fx = ModelFx;
    type In = MapIn;
    type Out = Id<Map>;
    const NAME: &'static str = "map";
    const VERSION: u32 = 1;

    async fn run(&self, ctx: &Ctx<ModelFx>, i: MapIn) -> Result<Id<Map>> {
        let prompt = render(TEMPLATE, &[("name", &i.name), ("problem", &i.problem)]);
        let raw: RawMap = ctx.ask_json(SYSTEM, &prompt).await?;

        // Valid when there are 2 to 8 parts, names are unique, and each part
        // has at least one condition.
        if !(2..=8).contains(&raw.parts.len()) {
            return Err(Error::BadModelOutput(format!(
                "a map needs 2 to 8 parts, got {}",
                raw.parts.len()
            )));
        }
        let mut names = BTreeSet::new();
        for p in &raw.parts {
            non_empty(&p.name, "part name")?;
            if !names.insert(p.name.clone()) {
                return Err(Error::BadModelOutput(format!("duplicate part name {:?}", p.name)));
            }
            if p.conditions.iter().all(|c| c.trim().is_empty()) {
                return Err(Error::BadModelOutput(format!(
                    "part {:?} has no conditions",
                    p.name
                )));
            }
        }
        ctx.put(
            Map {
                name: i.name,
                summary: raw.summary,
                parts: raw
                    .parts
                    .into_iter()
                    .map(|p| Part {
                        name: p.name,
                        description: p.description,
                        conditions: p.conditions,
                    })
                    .collect(),
            },
            &[],
        )
    }
}
