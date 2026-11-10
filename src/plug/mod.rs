//! The three places where the outside world enters. Each has a fake in
//! `fake.rs` that every test uses.

use std::collections::BTreeMap;

use async_trait::async_trait;

use crate::error::Result;
use crate::thing::{Grade, SourceItem, TrialSpec};

pub mod command_model;
pub mod command_verifier;
pub mod fake;
pub mod file_source;
pub mod handoff;

pub use command_model::CommandModel;
pub use command_verifier::{CommandVerifier, ParamTemplate, Template};
pub use fake::{FakeModel, FakeSource, FakeVerifier};
pub use file_source::FileSource;
pub use handoff::HandoffModel;

#[async_trait]
pub trait Model: Send + Sync {
    /// Part of the memo key for Model operations.
    fn id(&self) -> &str;
    async fn complete(&self, system: &str, prompt: &str, max_tokens: u32) -> Result<String>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceQuery {
    pub conditions: Vec<String>,
    pub exclude_domains: Vec<String>,
    pub k: usize,
}

#[async_trait]
pub trait Source: Send + Sync {
    fn name(&self) -> &str;
    async fn fetch(&self, q: &SourceQuery) -> Result<Vec<SourceItem>>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrialKind {
    pub name: String,
    pub params: Vec<String>,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrialOutcome {
    pub grade: Grade,
    pub observed: String,
    pub metrics: BTreeMap<String, f64>,
}

#[async_trait]
pub trait Verifier: Send + Sync {
    fn name(&self) -> &str;
    fn kinds(&self) -> Vec<TrialKind>;
    async fn run(&self, spec: &TrialSpec) -> Result<TrialOutcome>;
}
