//! Adapter for any OpenAI-compatible chat completions endpoint (feature `openai`):
//! OpenAI itself, and also Ollama, LM Studio, vLLM, OpenRouter, Together,
//! Groq, llama.cpp's server and the many others that speak the same protocol.
//!
//! The model name, the base URL and the name of the environment variable that
//! holds the key all come from config. Local servers usually need no key.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::Model;
use crate::error::{Error, Result};

pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

pub struct OpenAiModel {
    client: reqwest::Client,
    url: String,
    key: Option<String>,
    name: String,
    max_tokens_param: String,
}

impl OpenAiModel {
    /// * `base_url`: up to and including the version, e.g. `http://localhost:11434/v1`.
    /// * `key`: sent as a bearer token when present.
    /// * `max_tokens_param`: `max_tokens` for most servers; OpenAI's newer
    ///   reasoning models want `max_completion_tokens`.
    pub fn new(name: &str, base_url: &str, key: Option<String>, max_tokens_param: &str) -> Self {
        OpenAiModel {
            client: reqwest::Client::new(),
            url: format!("{}/chat/completions", base_url.trim_end_matches('/')),
            key,
            name: name.to_string(),
            max_tokens_param: max_tokens_param.to_string(),
        }
    }
}

#[async_trait]
impl Model for OpenAiModel {
    fn id(&self) -> &str {
        &self.name
    }

    async fn complete(&self, system: &str, prompt: &str, max_tokens: u32) -> Result<String> {
        let mut body = json!({
            "model": self.name,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": prompt },
            ],
        });
        body[&self.max_tokens_param] = json!(max_tokens);
        let mut req = self.client.post(&self.url).json(&body);
        if let Some(key) = &self.key {
            req = req.bearer_auth(key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| Error::Plug(format!("request to {} failed: {e}", self.url)))?;
        let status = resp.status();
        let value: Value = resp
            .json()
            .await
            .map_err(|e| Error::Plug(format!("reply from {} was not JSON ({status}): {e}", self.url)))?;
        if !status.is_success() {
            let msg = value["error"]["message"].as_str().unwrap_or("no message");
            return Err(Error::Plug(format!("{} {status}: {msg}", self.url)));
        }
        match value["choices"][0]["message"]["content"].as_str() {
            Some(text) if !text.trim().is_empty() => Ok(text.to_string()),
            _ => Err(Error::Plug(format!("reply from {} had no text", self.url))),
        }
    }
}
