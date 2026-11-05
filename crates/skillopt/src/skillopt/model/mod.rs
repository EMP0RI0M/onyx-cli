use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Default)]
pub struct ModelConfig {
    pub model_name: String,
    pub api_key: String,
    pub endpoint: String,
    pub temperature: Option<f64>,
    pub max_tokens: Option<i64>,
}

#[async_trait]
pub trait ChatBackend: Send + Sync {
    async fn generate_response(&self, messages: &[ChatMessage]) -> anyhow::Result<String>;
}

/// HTTP OpenAI-compatible backend using reqwest
pub struct OpenAICompatibleBackend {
    pub config: ModelConfig,
    client: reqwest::Client,
}

impl OpenAICompatibleBackend {
    pub fn new(config: ModelConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl ChatBackend for OpenAICompatibleBackend {
    async fn generate_response(&self, messages: &[ChatMessage]) -> anyhow::Result<String> {
        let payload = serde_json::json!({
            "model": self.config.model_name,
            "messages": messages,
            "temperature": self.config.temperature.unwrap_or(0.7),
            "max_tokens": self.config.max_tokens.unwrap_or(2048),
        });

        let mut req = self.client.post(&self.config.endpoint).json(&payload);
        if !self.config.api_key.is_empty() {
            req = req.bearer_auth(&self.config.api_key);
        }

        let resp = req.send().await?;
        let json: serde_json::Value = resp.json().await?;
        
        let reply = json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        Ok(reply)
    }
}
