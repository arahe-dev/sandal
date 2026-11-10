//! `.sandal/config.toml`: how model steps are answered, budget, sources,
//! verifier templates.
//!
//! SANDAL is a tool for harnesses (Claude Code, Codex, Cursor, a script, you),
//! not a harness. It holds no API keys and calls no model API: the harness that
//! drives it is the model, or it names a program SANDAL may run to ask one.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;

use crate::engine::{Budget, Plugs};
use crate::error::{Error, Result};
use crate::plug::command_verifier::{CommandVerifier, ParamTemplate, Template};
use crate::plug::{CommandModel, FakeModel, FileSource, HandoffModel, Model, Source};

pub const CONFIG_FILE: &str = "config.toml";

/// Written by `sandal init`.
pub const DEFAULT_CONFIG: &str = r#"[model]
# Who answers the model steps (map, back, match, carry, roots, read).
#
#   "handoff"   The harness driving SANDAL answers. SANDAL writes each prompt to
#               .sandal/handoff/, stops with exit code 3, and the harness replies
#               with `sandal answer <key>` and runs the command again. No model
#               API, no key, nothing to configure. This is the normal mode.
#   "command"   Let SANDAL ask a program on this machine, so a run needs no
#               hand-off: the prompt goes to its stdin and the reply is its
#               stdout. Point it at any harness CLI that has a one-shot mode.
#   "fake"      Always fails. For wiring checks.
provider = "handoff"
name = ""                     # a label for the command provider; part of the memo key
max_tokens = 2000             # passed to a command as SANDAL_MAX_TOKENS

# command = "my-harness"      # command provider: the program, and its fixed arguments
# args = ["--one-shot"]
# timeout_secs = 300

# The model that judges `sandal bench`. It should be at least as strong as the
# model under test. When this section is absent, [model] is used.
# [judge]
# provider = "handoff"

[budget]
max_model_calls = 200
max_world_calls = 50
max_concurrency = 8

[[source]]
kind = "file"
name = "notes"
path = "sources.jsonl"

[[verifier.template]]
name = "unit_test"
description = "Run one named test and report pass or fail"
program = "cargo"
args = ["test", "--quiet"]
timeout_secs = 120
  [[verifier.template.param]]
  name = "TEST_NAME"
  regex = "^[A-Za-z0-9_:]{1,80}$"
"#;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    pub provider: String,
    pub name: String,
    pub max_tokens: u32,
    /// command: the program and its fixed arguments.
    pub command: Option<String>,
    pub args: Vec<String>,
    pub timeout_secs: u64,
}

impl Default for ModelConfig {
    fn default() -> Self {
        ModelConfig {
            provider: "handoff".into(),
            name: String::new(),
            max_tokens: 2000,
            command: None,
            args: Vec::new(),
            timeout_secs: 300,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BudgetConfig {
    pub max_model_calls: u64,
    pub max_world_calls: u64,
    pub max_concurrency: usize,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        let b = Budget::default();
        BudgetConfig {
            max_model_calls: b.max_model_calls,
            max_world_calls: b.max_world_calls,
            max_concurrency: b.max_concurrency,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SourceConfig {
    pub kind: String,
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ParamConfig {
    pub name: String,
    pub regex: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TemplateConfig {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default, rename = "param")]
    pub params: Vec<ParamConfig>,
}

fn default_timeout() -> u64 {
    60
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct VerifierConfig {
    #[serde(rename = "template")]
    pub templates: Vec<TemplateConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub model: ModelConfig,
    pub judge: Option<ModelConfig>,
    pub budget: BudgetConfig,
    #[serde(rename = "source")]
    pub sources: Vec<SourceConfig>,
    pub verifier: VerifierConfig,
}

impl Config {
    pub fn parse(text: &str) -> Result<Config> {
        toml::from_str(text).map_err(|e| Error::Plug(format!("config: {e}")))
    }

    /// Let environment variables override `[model]`, so a harness can choose
    /// how model steps are answered without editing the file:
    /// `SANDAL_PROVIDER`, `SANDAL_COMMAND`.
    pub fn apply_env(mut self) -> Config {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        if let Some(v) = get("SANDAL_PROVIDER") {
            self.model.provider = v;
        }
        if let Some(v) = get("SANDAL_COMMAND") {
            self.model.command = Some(v);
        }
        self
    }

    /// Read `config.toml` from the state directory.
    pub fn load(dir: &Path) -> Result<Config> {
        let path = dir.join(CONFIG_FILE);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| Error::Plug(format!("cannot read {}: {e} (run `sandal init`?)", path.display())))?;
        Self::parse(&text)
    }

    pub fn budget(&self) -> Budget {
        Budget {
            max_model_calls: self.budget.max_model_calls,
            max_world_calls: self.budget.max_world_calls,
            max_concurrency: self.budget.max_concurrency,
        }
    }

    /// The model that judges the bench: `[judge]` when present, else `[model]`.
    pub fn judge_model_config(&self) -> &ModelConfig {
        self.judge.as_ref().unwrap_or(&self.model)
    }

    pub fn build_verifier(&self) -> Result<CommandVerifier> {
        CommandVerifier::new(
            self.verifier
                .templates
                .iter()
                .map(|t| Template {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    program: t.program.clone(),
                    args: t.args.clone(),
                    timeout_secs: t.timeout_secs,
                    params: t
                        .params
                        .iter()
                        .map(|p| ParamTemplate {
                            name: p.name.clone(),
                            regex: p.regex.clone(),
                        })
                        .collect(),
                })
                .collect(),
        )
    }

    /// Build the sources. Relative paths are resolved against `root`, the
    /// directory that holds `.sandal/`.
    pub fn build_sources(&self, root: &Path) -> Result<Vec<Arc<dyn Source>>> {
        self.sources
            .iter()
            .map(|s| match s.kind.as_str() {
                "file" => Ok(Arc::new(FileSource::new(&s.name, root.join(&s.path))) as Arc<dyn Source>),
                other => Err(Error::Plug(format!("unknown source kind {other:?}"))),
            })
            .collect()
    }

    /// Build the plug-ins. `need_model = false` is for commands that never ask
    /// a model anything. `state_dir` is `.sandal`.
    pub fn build_plugs(&self, root: &Path, state_dir: &Path, need_model: bool) -> Result<Plugs> {
        let model = if need_model {
            build_model(&self.model, state_dir)?
        } else {
            Arc::new(FakeModel::new(Vec::new())) as Arc<dyn Model>
        };
        Ok(Plugs {
            model,
            sources: self.build_sources(root)?,
            verifier: Arc::new(self.build_verifier()?),
        })
    }
}

pub fn build_model(cfg: &ModelConfig, state_dir: &Path) -> Result<Arc<dyn Model>> {
    match cfg.provider.as_str() {
        "handoff" => Ok(Arc::new(HandoffModel::new(state_dir))),
        "fake" => Ok(Arc::new(FakeModel::new(Vec::new()))),
        "command" => {
            let program = cfg
                .command
                .as_deref()
                .filter(|c| !c.trim().is_empty())
                .ok_or_else(|| {
                    Error::Plug("config: [model] command is required when provider = \"command\"".into())
                })?;
            Ok(Arc::new(CommandModel::new(
                &cfg.name,
                program,
                cfg.args.clone(),
                cfg.timeout_secs,
            )))
        }
        other => Err(Error::Plug(format!(
            "unknown model provider {other:?} (handoff, command, fake). SANDAL calls no model APIs: \
             the harness driving it answers, or `command` runs a program of yours"
        ))),
    }
}
