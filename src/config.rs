//! `.sandal/config.toml`: model, budget, sources, verifier templates.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;

use crate::engine::{Budget, Plugs};
use crate::error::{Error, Result};
use crate::plug::command_verifier::{CommandVerifier, ParamTemplate, Template};
use crate::plug::{FakeModel, FileSource, Model, Source};

pub const CONFIG_FILE: &str = "config.toml";

/// Written by `sandal init`.
pub const DEFAULT_CONFIG: &str = r#"[model]
# Which model SANDAL talks to. Pick one provider:
#   "handoff"    no API: prompts are written to .sandal/handoff/ and whoever is
#                driving SANDAL (an agent, or you) writes the replies. Needs nothing.
#   "openai"     any OpenAI-compatible endpoint: OpenAI, Ollama, LM Studio, vLLM,
#                OpenRouter, Groq, llama.cpp ... (set base_url; key is optional locally)
#   "anthropic"  the Anthropic Messages API
#   "command"    any program: the prompt goes to its stdin, the reply is its stdout
#   "fake"       always fails; for wiring checks
provider = "handoff"
name = ""                     # the model name (openai, anthropic); a label for command
max_tokens = 2000

# base_url = "http://localhost:11434/v1"   # openai: default https://api.openai.com/v1
# api_key_env = "OPENAI_API_KEY"           # name of the env var holding the key
# max_tokens_param = "max_tokens"          # openai: "max_completion_tokens" for OpenAI reasoning models
# command = "claude"                       # command provider: program and fixed arguments
# args = ["-p"]
# timeout_secs = 300

# The model that judges `sandal bench`. It should be at least as strong as the
# model under test. When this section is absent, [model] is used.
# [judge]
# provider = "openai"
# name = ""

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
    /// openai and anthropic: where to send requests.
    pub base_url: Option<String>,
    /// Name of the environment variable that holds the API key.
    pub api_key_env: Option<String>,
    /// openai: the request field that carries the token limit.
    pub max_tokens_param: Option<String>,
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
            base_url: None,
            api_key_env: None,
            max_tokens_param: None,
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

    /// Let environment variables override `[model]`, so an agent can pick a
    /// model without editing the file: `SANDAL_PROVIDER`, `SANDAL_MODEL`,
    /// `SANDAL_BASE_URL`, `SANDAL_API_KEY_ENV`.
    pub fn apply_env(mut self) -> Config {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        if let Some(v) = get("SANDAL_PROVIDER") {
            self.model.provider = v;
        }
        if let Some(v) = get("SANDAL_MODEL") {
            self.model.name = v;
        }
        if let Some(v) = get("SANDAL_BASE_URL") {
            self.model.base_url = Some(v);
        }
        if let Some(v) = get("SANDAL_API_KEY_ENV") {
            self.model.api_key_env = Some(v);
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

    /// Build the plug-ins. `need_model = false` is for commands that never call
    /// the model, so they work without a key. `state_dir` is `.sandal`.
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

/// The key from the named environment variable, if it is set and not empty.
#[cfg(any(feature = "openai", feature = "anthropic"))]
fn key_from(env_name: &str) -> Option<String> {
    std::env::var(env_name).ok().filter(|k| !k.trim().is_empty())
}

pub fn build_model(cfg: &ModelConfig, state_dir: &Path) -> Result<Arc<dyn Model>> {
    let need_name = |what: &str| -> Result<()> {
        if cfg.name.trim().is_empty() {
            Err(Error::Plug(format!(
                "config: [model] name is required when provider = \"{what}\""
            )))
        } else {
            Ok(())
        }
    };
    match cfg.provider.as_str() {
        "fake" => Ok(Arc::new(FakeModel::new(Vec::new()))),
        "handoff" => Ok(Arc::new(crate::plug::HandoffModel::new(state_dir))),
        "command" => {
            let program = cfg
                .command
                .as_deref()
                .filter(|c| !c.trim().is_empty())
                .ok_or_else(|| {
                    Error::Plug("config: [model] command is required when provider = \"command\"".into())
                })?;
            Ok(Arc::new(crate::plug::CommandModel::new(
                &cfg.name,
                program,
                cfg.args.clone(),
                cfg.timeout_secs,
            )))
        }
        "openai" => {
            need_name("openai")?;
            #[cfg(feature = "openai")]
            {
                use crate::plug::openai::{OpenAiModel, DEFAULT_BASE_URL};
                let base = cfg.base_url.as_deref().unwrap_or(DEFAULT_BASE_URL);
                let key_env = cfg.api_key_env.as_deref().unwrap_or("OPENAI_API_KEY");
                let key = key_from(key_env);
                // Only the real OpenAI endpoint insists on a key; local and
                // self-hosted servers usually take none.
                if key.is_none() && base == DEFAULT_BASE_URL {
                    return Err(Error::Plug(format!("{key_env} is not set")));
                }
                Ok(Arc::new(OpenAiModel::new(
                    &cfg.name,
                    base,
                    key,
                    cfg.max_tokens_param.as_deref().unwrap_or("max_tokens"),
                )))
            }
            #[cfg(not(feature = "openai"))]
            {
                Err(Error::Plug(
                    "this build has no OpenAI-compatible adapter: rebuild with `--features openai`".into(),
                ))
            }
        }
        "anthropic" => {
            need_name("anthropic")?;
            #[cfg(feature = "anthropic")]
            {
                use crate::plug::anthropic::{AnthropicModel, DEFAULT_BASE_URL};
                let base = cfg.base_url.as_deref().unwrap_or(DEFAULT_BASE_URL);
                let key_env = cfg.api_key_env.as_deref().unwrap_or("ANTHROPIC_API_KEY");
                let key = key_from(key_env);
                if key.is_none() && base == DEFAULT_BASE_URL {
                    return Err(Error::Plug(format!("{key_env} is not set")));
                }
                Ok(Arc::new(AnthropicModel::new(&cfg.name, base, key)))
            }
            #[cfg(not(feature = "anthropic"))]
            {
                Err(Error::Plug(
                    "this build has no Anthropic adapter: rebuild with `--features anthropic`".into(),
                ))
            }
        }
        other => Err(Error::Plug(format!(
            "unknown model provider {other:?} (handoff, openai, anthropic, command, fake)"
        ))),
    }
}
