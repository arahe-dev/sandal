use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{Back, BackIn, Carry, CarryIn, MatchIn, MatchOp, Mix, MixIn, Notice, NoticeIn, Roots, RootsIn};
use crate::ctx::Ctx;
use crate::error::Result;
use crate::fx::WorldFx;
use crate::id::Id;
use crate::op::{keep_ok, Op};
use crate::thing::{Map, Study};

/// A World operation built only from the ones above. It shows the composition
/// style and is what the CLI runs. It does not run trials: those are started
/// explicitly with `Try`.
pub struct Cycle;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CycleIn {
    pub map: Id<Map>,
    pub k: usize,
    pub max_trials: usize,
    pub exclude_domains: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CycleOut {
    pub study: Id<Study>,
    pub skipped: u32,
}

#[async_trait]
impl Op for Cycle {
    type Fx = WorldFx;
    type In = CycleIn;
    type Out = CycleOut;
    const NAME: &'static str = "cycle";
    const VERSION: u32 = 1;

    async fn run(&self, ctx: &Ctx<WorldFx>, i: CycleIn) -> Result<CycleOut> {
        // Failures of single items are dropped and counted, not fatal.
        let mut skipped = 0u32;
        let sources = ctx
            .run(&Notice, NoticeIn { map: i.map, k: i.k, exclude_domains: i.exclude_domains })
            .await?;
        let origins = keep_ok(
            ctx.run_all(&Back, sources.iter().map(|s| BackIn { thing: s.raw() }).collect()).await,
            &mut skipped,
        )?;
        let matches = keep_ok(
            ctx.run_all(&MatchOp, origins.iter().map(|o| MatchIn { origin: *o, map: i.map }).collect()).await,
            &mut skipped,
        )?;
        let props = keep_ok(
            ctx.run_all(&Carry, matches.into_iter().flatten().map(|m| CarryIn { m }).collect()).await,
            &mut skipped,
        )?;
        let labels = ctx.run(&Roots, RootsIn { proposals: props }).await?;
        let study = ctx.run(&Mix, MixIn { labels, max_trials: i.max_trials }).await?;
        Ok(CycleOut { study, skipped })
    }
}
