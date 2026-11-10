//! Adapter for the Anthropic Messages API (feature `anthropic`).
//!
//! The key comes from `ANTHROPIC_API_KEY` and the model name from config.
//! No model name is hard-coded here.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::Model;
use crate::error::{Error, Result};

const URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

pub struct AnthropicModel {
    client: reqwest::Client,
    key: String,
    name: String,
}

impl AnthropicModel {
    pub fn from_env(name: &str) -> Result<Self> {
        let key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| Error::Plug("ANTHROPIC_API_KEY is not set".into()))?;
        Ok(AnthropicModel {
            client: reqwest::Client::new(),
            key,
            name: name.to_string(),
        })
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
        let resp = self
            .client
            .post(URL)
            .header("x-api-key", &self.key)
            .header("anthropic-version", API_VERSION)
            .json(&body)
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
