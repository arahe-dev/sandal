//! Adapter for the Anthropic Messages API (feature `anthropic`).
//!
//! The model name, the base URL and the name of the environment variable that
//! holds the key all come from config. No model name is hard-coded here.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::Model;
use crate::error::{Error, Result};

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";

pub struct AnthropicModel {
    client: reqwest::Client,
    url: String,
    key: Option<String>,
    name: String,
}

impl AnthropicModel {
    /// `key` is sent as `x-api-key` when present. `base_url` is the host,
    /// without `/v1/messages`.
    pub fn new(name: &str, base_url: &str, key: Option<String>) -> Self {
        AnthropicModel {
            client: reqwest::Client::new(),
            url: format!("{}/v1/messages", base_url.trim_end_matches('/')),
            key,
            name: name.to_string(),
        }
    }
}

#[async_trait]
impl Model for AnthropicModel {
    fn id(&self) -> &str {
        &self.name
    }

    async fn complete(&self, system: &str, prompt: &str, max_tokens: u32) -> Result<String> {
        let body = json!({
            "model": self.name,
            "max_tokens": max_tokens,
            "system": system,
            "messages": [{ "role": "user", "content": prompt }],
        });
        let mut req = self
            .client
            .post(&self.url)
            .header("anthropic-version", API_VERSION)
            .json(&body);
        if let Some(key) = &self.key {
            req = req.header("x-api-key", key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| Error::Plug(format!("anthropic request failed: {e}")))?;
        let status = resp.status();
        let value: Value = resp
            .json()
            .await
            .map_err(|e| Error::Plug(format!("anthropic reply was not JSON ({status}): {e}")))?;
        if !status.is_success() {
            let msg = value["error"]["message"].as_str().unwrap_or("no message");
            return Err(Error::Plug(format!("anthropic {status}: {msg}")));
        }
        // The reply is a list of content blocks; the text ones make up the answer.
        let text: String = value["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect();
        if text.is_empty() {
            return Err(Error::Plug("anthropic reply had no text".into()));
        }
        Ok(text)
    }
}
