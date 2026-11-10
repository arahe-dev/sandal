use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{bullets, describe, non_empty, render, SYSTEM};
use crate::ctx::Ctx;
use crate::error::{Error, Result};
use crate::fx::ModelFx;
use crate::id::Id;
use crate::op::Op;
use crate::thing::{Match, Proposal, TrialSpec};

/// Rebuilds the source idea for one part of the target and names the trial.
pub struct Carry;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CarryIn {
    pub m: Id<Match>,
}

#[derive(Deserialize)]
struct RawCarry {
    rebuilt: String,
    root: String,
    trial: RawTrial,
    if_right: String,
    if_wrong: String,
}

#[derive(Deserialize)]
struct RawTrial {
    kind: String,
    params: BTreeMap<String, String>,
    description: String,
}

const TEMPLATE: &str = include_str!("../prompts/carry.txt");

#[async_trait]
impl Op for Carry {
    type Fx = ModelFx;
    type In = CarryIn;
    type Out = Id<Proposal>;
    const NAME: &'static str = "carry";
    // The set of trial kinds comes from the verifier in use. It is not part of
    // the memo key, so bump this when a verifier's templates change meaning.
    const VERSION: u32 = 1;

    async fn run(&self, ctx: &Ctx<ModelFx>, i: CarryIn) -> Result<Id<Proposal>> {
        let m = ctx.get(i.m)?;
        let origin = ctx.get(m.origin)?;
        let source = ctx.thing(origin.of)?;
        let map = ctx.get(m.map)?;
        let part = map
            .parts
            .iter()
            .find(|p| p.name == m.part)
            .ok_or_else(|| Error::NotFound(format!("part {:?} in map {}", m.part, map.name)))?;

        // Unlike `back` and `match`, this prompt includes the source's outcome when present.
        let source_text = describe(&source)?.text(true);
        let kinds = ctx.trial_kinds();
        let kinds_text = kinds
            .iter()
            .map(|k| {
                format!(
                    "- {} (parameters: {}): {}",
                    k.name,
                    if k.params.is_empty() {
                        "none".to_string()
                    } else {
                        k.params.join(", ")
                    },
                    k.description
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let prompt = render(
            TEMPLATE,
            &[
                ("source", &source_text),
                ("principles", &bullets(&origin.principles)),
                ("part", &part.name),
                ("part_description", &part.description),
                ("part_conditions", &bullets(&part.conditions)),
                ("shared", &bullets(&m.shared)),
                ("missing", &bullets(&m.missing)),
                ("trial_kinds", &kinds_text),
            ],
        );
        let raw: RawCarry = ctx.ask_json(SYSTEM, &prompt).await?;

        // Valid when trial.kind is one the verifier accepts and every field is non-empty.
        non_empty(&raw.rebuilt, "rebuilt")?;
        non_empty(&raw.root, "root")?;
        non_empty(&raw.trial.kind, "trial.kind")?;
        non_empty(&raw.trial.description, "trial.description")?;
        non_empty(&raw.if_right, "if_right")?;
        non_empty(&raw.if_wrong, "if_wrong")?;
        if !kinds.iter().any(|k| k.name == raw.trial.kind) {
            return Err(Error::BadModelOutput(format!(
                "trial kind {:?} is not one the verifier accepts",
                raw.trial.kind
            )));
        }
        ctx.put(
            Proposal {
                source: origin.of,
                origin: m.origin,
                map: m.map,
                part: m.part,
                rebuilt: raw.rebuilt,
                root: raw.root,
                trial: TrialSpec {
                    kind: raw.trial.kind,
                    params: raw.trial.params,
                    description: raw.trial.description,
                },
                if_right: raw.if_right,
                if_wrong: raw.if_wrong,
            },
            &[i.m.raw(), origin.of],
        )
    }
}
