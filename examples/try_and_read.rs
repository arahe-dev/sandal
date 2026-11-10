//! The reference for writing a new operation.
//!
//! `TryAndRead` tries the first few proposals of a study, then re-reads each
//! proposal's source. Anything a user wants to add looks like this: a struct
//! that implements `Op` and calls other operations through its `Ctx`. The
//! compiler checks that the effects line up (this one is `WorldFx`, so it may
//! call `Try`, which is World, and `Read`, which is Model).
//!
//! Run it with: cargo run --example try_and_read

use async_trait::async_trait;
use sandal::*;
use serde::{Deserialize, Serialize};

#[path = "../tests/common/world.rs"]
mod world;

pub struct TryAndRead;

#[derive(Clone, Serialize, Deserialize)]
pub struct TryAndReadIn {
    pub study: Id<Study>,
    pub map: Id<Map>,
    pub n: usize,
}

#[async_trait]
impl Op for TryAndRead {
    type Fx = WorldFx;
    type In = TryAndReadIn;
    type Out = Vec<Id<Reading>>;
    const NAME: &'static str = "try_and_read";
    const VERSION: u32 = 1;

    async fn run(&self, ctx: &Ctx<WorldFx>, i: TryAndReadIn) -> Result<Self::Out> {
        let study = ctx.get(i.study)?;
        let chosen: Vec<_> = study.order.into_iter().take(i.n).collect();
        each(ctx, &Try, chosen.iter().map(|p| TryIn { proposal: *p }).collect()).await;
        let mut out = Vec::new();
        for p in chosen {
            let source = ctx.get(p)?.source;
            out.push(ctx.run(&Read, ReadIn { thing: source, map: i.map }).await?);
        }
        Ok(out)
    }
}

#[allow(dead_code)]
#[tokio::main]
async fn main() -> Result<()> {
    // The scripted world from the tests: a model, a source and a verifier that
    // all answer from fixed scripts, so this runs offline.
    let w = world::world();
    let map = world::make_map(&w).await;
    let cycle = w
        .engine
        .run(&Cycle, CycleIn { map, k: 10, max_trials: 5, exclude_domains: vec![] })
        .await?;
    let readings = w
        .engine
        .run(&TryAndRead, TryAndReadIn { study: cycle.study, map, n: 2 })
        .await?;
    for r in readings {
        let reading = w.engine.get(r)?;
        println!("{}: {}", r.raw().short(), reading.text);
    }
    println!("{:?}", w.engine.stats());
    Ok(())
}
