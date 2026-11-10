use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::ctx::Ctx;
use crate::error::Result;
use crate::fx::WorldFx;
use crate::id::Id;
use crate::op::Op;
use crate::thing::{Proposal, TrialResult};

/// Runs a proposal's trial through the verifier.
pub struct Try;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TryIn {
    pub proposal: Id<Proposal>,
}

#[async_trait]
impl Op for Try {
    type Fx = WorldFx;
    type In = TryIn;
    type Out = Id<TrialResult>;
    const NAME: &'static str = "try";
    const VERSION: u32 = 1;

    /// A verifier error or timeout still appends a TrialResult, graded
    /// Uncertain, with the error text in `observed` (see `Ctx::verify`).
    async fn run(&self, ctx: &Ctx<WorldFx>, i: TryIn) -> Result<Id<TrialResult>> {
        ctx.verify(i.proposal).await
    }
}
