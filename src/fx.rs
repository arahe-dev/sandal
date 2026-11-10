use crate::thing::Effect;

/// Effect markers. They are types, so the compiler checks which operations may
/// call which.
pub trait Fx: Send + Sync + 'static {
    const EFFECT: Effect;
}

pub struct PureFx;
pub struct ModelFx;
pub struct WorldFx;

impl Fx for PureFx {
    const EFFECT: Effect = Effect::Pure;
}
impl Fx for ModelFx {
    const EFFECT: Effect = Effect::Model;
}
impl Fx for WorldFx {
    const EFFECT: Effect = Effect::World;
}

/// `Outer: Allows<Inner>` means an Outer operation may call an Inner one.
pub trait Allows<Inner: Fx>: Fx {}
impl Allows<PureFx> for PureFx {}
impl Allows<PureFx> for ModelFx {}
impl Allows<ModelFx> for ModelFx {}
impl Allows<PureFx> for WorldFx {}
impl Allows<ModelFx> for WorldFx {}
impl Allows<WorldFx> for WorldFx {}
