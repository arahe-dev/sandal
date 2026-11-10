use async_trait::async_trait;
use sandal::*;

struct Bad;

#[async_trait]
impl Op for Bad {
    type Fx = PureFx;
    type In = TryIn;
    type Out = Id<TrialResult>;
    const NAME: &'static str = "bad";
    const VERSION: u32 = 1;

    async fn run(&self, ctx: &Ctx<PureFx>, i: TryIn) -> Result<Id<TrialResult>> {
        // A Pure operation may not call a World operation.
        ctx.run(&Try, i).await
    }
}

fn main() {}
