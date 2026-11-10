use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::ctx::Ctx;
use crate::error::Result;
use crate::fx::WorldFx;
use crate::id::Id;
use crate::op::Op;
use crate::plug::SourceQuery;
use crate::thing::{Map, SourceItem};

/// Brings in material from outside.
pub struct Notice;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NoticeIn {
    pub map: Id<Map>,
    pub k: usize,
    pub exclude_domains: Vec<String>,
}

#[async_trait]
impl Op for Notice {
    type Fx = WorldFx;
    type In = NoticeIn;
    type Out = Vec<Id<SourceItem>>;
    const NAME: &'static str = "notice";
    const VERSION: u32 = 1;

    async fn run(&self, ctx: &Ctx<WorldFx>, i: NoticeIn) -> Result<Vec<Id<SourceItem>>> {
        let map = ctx.get(i.map)?;
        let conditions = map
            .parts
            .iter()
            .flat_map(|p| p.conditions.iter().cloned())
            .collect();
        ctx.fetch(&SourceQuery {
            conditions,
            exclude_domains: i.exclude_domains,
            k: i.k,
        })
        .await
    }
}
