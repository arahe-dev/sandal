//! A model that is the agent (or person) driving SANDAL.
//!
//! No API, no key. When SANDAL needs a completion it writes the prompt to a
//! file and stops with `Error::Pending`. The caller answers by writing the
//! reply to the matching response file and runs the same command again:
//! everything already answered is read back from disk, and finished operations
//! are memoised, so a rerun only does the new work.
//!
//! ```text
//! .sandal/handoff/<key>.prompt.txt     written by SANDAL
//! .sandal/handoff/<key>.response.txt   written by the caller
//! ```
//!
//! `<key>` is a hash of the system prompt and the prompt, so the same question
//! always lands in the same files.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::Serialize;

use super::Model;
use crate::error::{Error, Result};
use crate::id::sha256;

pub const HANDOFF_DIR: &str = "handoff";

pub struct HandoffModel {
    dir: PathBuf,
}

/// One prompt still waiting for an answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PendingPrompt {
    pub key: String,
    /// The prompt itself, so a caller need not open the file.
    pub prompt: String,
    pub prompt_file: PathBuf,
    pub response_file: PathBuf,
}

impl HandoffModel {
    /// `state_dir` is the `.sandal` directory.
    pub fn new(state_dir: &Path) -> Self {
        HandoffModel {
            dir: handoff_dir(state_dir),
        }
    }

    fn key(system: &str, prompt: &str) -> String {
        hex::encode(&sha256(&[system.as_bytes(), prompt.as_bytes()])[..12])
    }

    fn files(&self, key: &str) -> (PathBuf, PathBuf) {
        (
            self.dir.join(format!("{key}.prompt.txt")),
            self.dir.join(format!("{key}.response.txt")),
        )
    }
}

/// The handoff directory as an absolute path, so the files named in messages
/// can be opened by a caller working from any directory.
fn handoff_dir(state_dir: &Path) -> PathBuf {
    std::path::absolute(state_dir)
        .unwrap_or_else(|_| state_dir.to_path_buf())
        .join(HANDOFF_DIR)
}

/// The prompts in `state_dir` that have no response yet, sorted by key.
pub fn pending(state_dir: &Path) -> Vec<PendingPrompt> {
    let dir = handoff_dir(state_dir);
    let mut out: Vec<PendingPrompt> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let key = name.strip_suffix(".prompt.txt")?.to_string();
            let response_file = dir.join(format!("{key}.response.txt"));
            (!response_file.exists()).then(|| PendingPrompt {
                key,
                prompt: std::fs::read_to_string(e.path()).unwrap_or_default(),
                prompt_file: e.path(),
                response_file,
            })
        })
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

/// Record the reply to a pending prompt. `key` may be any unique prefix of the
/// key. Returns the prompt it answered.
pub fn answer(state_dir: &Path, key: &str, reply: &str) -> Result<PendingPrompt> {
    if reply.trim().is_empty() {
        return Err(Error::Plug("the reply is empty".into()));
    }
    let list = pending(state_dir);
    let mut hits = list.iter().filter(|p| p.key.starts_with(key));
    let found = match (hits.next(), hits.next()) {
        (Some(p), None) => p.clone(),
        (None, _) => {
            return Err(Error::NotFound(format!(
                "no pending prompt starts with {key:?} (see `sandal pending`)"
            )))
        }
        (Some(_), Some(_)) => return Err(Error::NotFound(format!("prefix {key:?} is ambiguous"))),
    };
    std::fs::write(&found.response_file, reply.trim_end())?;
    Ok(found)
}

#[async_trait]
impl Model for HandoffModel {
    fn id(&self) -> &str {
        "handoff"
    }

    async fn complete(&self, system: &str, prompt: &str, max_tokens: u32) -> Result<String> {
        let key = Self::key(system, prompt);
        let (prompt_file, response_file) = self.files(&key);
        match std::fs::read_to_string(&response_file) {
            Ok(reply) if !reply.trim().is_empty() => return Ok(reply),
            Ok(_) => {} // an empty file is "not answered yet"
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(
            &prompt_file,
            format!(
                "# SANDAL needs a reply to this prompt (at most {max_tokens} tokens).\n\
                 # Write ONLY the reply to: {}\n\n\
                 ## System\n{system}\n\n## Prompt\n{prompt}\n",
                response_file.display()
            ),
        )?;
        Err(Error::Pending(format!(
            "write the reply to {} (prompt in {})",
            response_file.display(),
            prompt_file.display()
        )))
    }
}
