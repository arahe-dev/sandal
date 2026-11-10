//! The `Op` trait and three helpers for composing operations.

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::ctx::Ctx;
use crate::error::{Error, Result};
use crate::fx::{Allows, Fx};

#[async_trait]
pub trait Op: Send + Sync + 'static {
    type Fx: Fx;
    type In: Serialize + DeserializeOwned + Clone + Send + Sync;
    type Out: Serialize + DeserializeOwned + Clone + Send + Sync;
    const NAME: &'static str;
    const VERSION: u32; // bump when the logic or the prompt changes

    /// Put an input in canonical form before its memo key is computed, so
    /// inputs that mean the same thing share an entry (for example a list whose
    /// order does not matter is sorted).
    ///
    /// DECISION: the document asks for this ("input ids are sorted before the
    /// memo key is computed") without naming a hook; a defaulted method is the
    /// smallest way to give an operation that power.
    fn normalize(_input: &mut Self::In) {}

    async fn run(&self, ctx: &Ctx<Self::Fx>, input: Self::In) -> Result<Self::Out>;
}

/// `a`, then `b` on a's output.
pub async fn then<E, A, B>(ctx: &Ctx<E>, a: &A, b: &B, input: A::In) -> Result<B::Out>
where
    E: Allows<A::Fx> + Allows<B::Fx>,
    A: Op,
    B: Op<In = A::Out>,
{
    let mid = ctx.run(a, input).await?;
    ctx.run(b, mid).await
}

/// `a` on every input, concurrently. Same as `ctx.run_all`.
pub async fn each<E, A>(ctx: &Ctx<E>, a: &A, inputs: Vec<A::In>) -> Vec<Result<A::Out>>
where
    E: Allows<A::Fx>,
    A: Op,
{
    ctx.run_all(a, inputs).await
}

/// Run `a`, ask `next` for the following input, repeat. Stops when `next` returns
/// `None` or after `max_rounds`. Returns every output in order.
pub async fn iterate<E, A, F>(
    ctx: &Ctx<E>,
    a: &A,
    start: A::In,
    next: F,
    max_rounds: usize,
) -> Result<Vec<A::Out>>
where
    E: Allows<A::Fx>,
    A: Op,
    F: Fn(&A::Out) -> Option<A::In> + Send + Sync,
{
    let mut outs = Vec::new();
    let mut input = start;
    for _ in 0..max_rounds {
        let out = ctx.run(a, input).await?;
        let following = next(&out);
        outs.push(out);
        match following {
            Some(i) => input = i,
            None => break,
        }
    }
    Ok(outs)
}

/// Used where a composed operation drops failures and carries on: keeps the
/// successes and counts the failures.
///
/// DECISION: `BudgetExhausted` is never skipped. Dropping it would make a run
/// that ran out of budget look like one where the model had nothing to say.
pub fn keep_ok<T>(results: Vec<Result<T>>, skipped: &mut u32) -> Result<Vec<T>> {
    let mut out = Vec::new();
    for r in results {
        match r {
            Ok(v) => out.push(v),
            Err(e @ Error::BudgetExhausted(_)) => return Err(e),
            Err(_) => *skipped += 1,
        }
    }
    Ok(out)
}
