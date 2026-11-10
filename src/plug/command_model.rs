//! A model that is any program on the machine: a local CLI agent, a script, a
//! wrapper around some other API. The prompt goes to the program's standard
//! input and the reply is its standard output.
//!
//! The system prompt is sent first, then a blank line, then the prompt, so
//! programs that take only plain text on stdin still see both. The program and
//! its arguments come from config and are fixed; no model output is ever used
//! to build a command.

use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::Model;
use crate::error::{Error, Result};

pub struct CommandModel {
    id: String,
    program: String,
    args: Vec<String>,
    timeout: Duration,
}

impl CommandModel {
    /// `name` (from config) is part of the memo key, so changing the model
    /// behind a command also changes the key.
    pub fn new(name: &str, program: &str, args: Vec<String>, timeout_secs: u64) -> Self {
        CommandModel {
            id: if name.is_empty() {
                format!("command:{program}")
            } else {
                format!("command:{name}")
            },
            program: program.to_string(),
            args,
            timeout: Duration::from_secs(timeout_secs),
        }
    }
}

#[async_trait]
impl Model for CommandModel {
    fn id(&self) -> &str {
        &self.id
    }

    async fn complete(&self, system: &str, prompt: &str, max_tokens: u32) -> Result<String> {
        let mut child = Command::new(&self.program)
            .args(&self.args)
            .env("SANDAL_MAX_TOKENS", max_tokens.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| Error::Plug(format!("could not start {}: {e}", self.program)))?;
        let mut stdin = child.stdin.take().expect("stdin was piped");
        let input = format!("{system}\n\n{prompt}");
        // Write on a separate task so a program that replies before reading
        // everything cannot deadlock us.
        let writer = tokio::spawn(async move {
            let _ = stdin.write_all(input.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });
        let output = tokio::time::timeout(self.timeout, child.wait_with_output())
            .await
            .map_err(|_| Error::Plug(format!("{} timed out after {} s", self.program, self.timeout.as_secs())))?
            .map_err(|e| Error::Plug(format!("waiting for {}: {e}", self.program)))?;
        let _ = writer.await;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Plug(format!(
                "{} exited with {}: {}",
                self.program,
                output.status,
                err.trim().chars().take(300).collect::<String>()
            )));
        }
        let reply = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if reply.is_empty() {
            return Err(Error::Plug(format!("{} printed nothing", self.program)));
        }
        Ok(reply)
    }
}
