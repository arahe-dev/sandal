use async_trait::async_trait;
use sandal::*;

struct Bad;

#[async_trait]
impl Op for Bad {
    type Fx = PureFx;
    type In = ();
    type Out = String;
    const NAME: &'static str = "bad";
    const VERSION: u32 = 1;

    async fn run(&self, ctx: &Ctx<PureFx>, _: ()) -> Result<String> {
        // A Pure operation may not call the model.
        ctx.ask("system", "prompt").await
    }
}

fn main() {}
