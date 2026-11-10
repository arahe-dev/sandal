//! A verifier that runs command templates the user wrote in config.
//!
//! Proposals are written by a model, so a model must never be able to supply a
//! command. It can only pick a template by name and fill in named parameters.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use regex::Regex;
use tokio::process::Command;

use super::{TrialKind, TrialOutcome, Verifier};
use crate::error::{Error, Result};
use crate::thing::{Grade, TrialSpec};

/// One declared parameter and the regex its value must match.
#[derive(Clone, Debug)]
pub struct ParamTemplate {
    pub name: String,
    pub regex: String,
}

/// A template: a fixed program with fixed arguments, a timeout, and named parameters.
#[derive(Clone, Debug)]
pub struct Template {
    pub name: String,
    pub description: String,
    pub program: String,
    pub args: Vec<String>,
    pub timeout_secs: u64,
    pub params: Vec<ParamTemplate>,
}

struct Compiled {
    template: Template,
    regexes: Vec<Regex>,
}

pub struct CommandVerifier {
    templates: Vec<Compiled>,
}

impl CommandVerifier {
    /// Fails if a parameter regex does not compile.
    pub fn new(templates: Vec<Template>) -> Result<Self> {
        let templates = templates
            .into_iter()
            .map(|template| {
                let regexes = template
                    .params
                    .iter()
                    .map(|p| {
                        Regex::new(&p.regex).map_err(|e| {
                            Error::Plug(format!(
                                "template {}: bad regex for {}: {e}",
                                template.name, p.name
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(Compiled { template, regexes })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(CommandVerifier { templates })
    }
}

#[async_trait]
impl Verifier for CommandVerifier {
    fn name(&self) -> &str {
        "command"
    }

    fn kinds(&self) -> Vec<TrialKind> {
        self.templates
            .iter()
            .map(|c| TrialKind {
                name: c.template.name.clone(),
                params: c.template.params.iter().map(|p| p.name.clone()).collect(),
                description: c.template.description.clone(),
            })
            .collect()
    }

    async fn run(&self, spec: &TrialSpec) -> Result<TrialOutcome> {
        let compiled = self
            .templates
            .iter()
            .find(|c| c.template.name == spec.kind)
            .ok_or_else(|| Error::Plug(format!("unknown trial kind {:?}", spec.kind)))?;
        let t = &compiled.template;

        // Validate every parameter before anything is started.
        let mut env: Vec<(String, String)> = Vec::new();
        for (p, re) in t.params.iter().zip(&compiled.regexes) {
            let value = spec
                .params
                .get(&p.name)
                .ok_or_else(|| Error::Plug(format!("missing parameter {}", p.name)))?;
            if !re.is_match(value) {
                return Err(Error::Plug(format!(
                    "parameter {} does not match its pattern",
                    p.name
                )));
            }
            env.push((format!("SANDAL_PARAM_{}", p.name), value.clone()));
        }

        // Never through a shell: the program and its arguments are fixed, and
        // parameters reach it only as environment variables.
        let mut cmd = Command::new(&t.program);
        cmd.args(&t.args)
            .envs(env)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let child = cmd
            .spawn()
            .map_err(|e| Error::Plug(format!("could not start {}: {e}", t.program)))?;
        let output = tokio::time::timeout(
            Duration::from_secs(t.timeout_secs),
            child.wait_with_output(),
        )
        .await
        .map_err(|_| Error::Plug(format!("timed out after {} s", t.timeout_secs)))?
        .map_err(|e| Error::Plug(format!("waiting for {}: {e}", t.program)))?;

        let grade = match output.status.code() {
            Some(0) => Grade::Confirmed,
            Some(2) => Grade::Partial,
            _ => Grade::Refuted,
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut metrics = BTreeMap::new();
        for line in stdout.lines() {
            let mut it = line.split_whitespace();
            if let (Some("metric"), Some(name), Some(num), None) =
                (it.next(), it.next(), it.next(), it.next())
            {
                if let Ok(v) = num.parse::<f64>() {
                    metrics.insert(name.to_string(), v);
                }
            }
        }
        let chars: Vec<char> = stdout.chars().collect();
        let observed: String = chars[chars.len().saturating_sub(2000)..].iter().collect();
        Ok(TrialOutcome {
            grade,
            observed,
            metrics,
        })
    }
}
