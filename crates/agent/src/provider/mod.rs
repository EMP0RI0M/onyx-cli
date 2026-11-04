//! Multi-Provider Registry, Configuration & Authentication subsystem.
//! Powered by catalog definitions from liter-llm (174+ LLM providers, OAuth Device flow,
//! API keys, and custom endpoints).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Static embedded providers catalog from liter-llm.
pub const PROVIDERS_CATALOG_JSON: &str = include_str!("providers_catalog.json");

/// Provider metadata specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSpec {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub env_var: Option<String>,
    #[serde(default = "default_auth_type")]
    pub auth_type: String,
    #[serde(default = "default_streaming_format")]
    pub streaming_format: String,
    #[serde(default)]
    pub model_prefixes: Vec<String>,
    #[serde(default)]
    pub model_count: usize,
    #[serde(default)]
    pub function_calling: bool,
    #[serde(default)]
    pub vision: bool,
    #[serde(default)]
    pub reasoning: bool,
}

fn default_auth_type() -> String {
    "bearer".to_string()
}

fn default_streaming_format() -> String {
    "sse".to_string()
}

/// Active user configuration for an individual provider stored in `~/.onyx/providers.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider_id: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub oauth_token: Option<String>,
    #[serde(default)]
    pub active_model: Option<String>,
    #[serde(default)]
    pub custom_headers: HashMap<String, String>,
}

/// Stored configuration root in `~/.onyx/providers.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProvidersStorage {
    #[serde(default = "default_active_provider")]
    pub active_provider: String,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
}

fn default_active_provider() -> String {
    "custom_openai".to_string()
}

/// Directory path `~/.onyx/`
pub fn onyx_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".onyx")
}

/// Path to `~/.onyx/providers.json`
pub fn providers_path() -> PathBuf {
    onyx_dir().join("providers.json")
}

/// Load the entire provider catalog from the embedded schema.
pub fn load_catalog() -> Vec<ProviderSpec> {
    serde_json::from_str(PROVIDERS_CATALOG_JSON).unwrap_or_default()
}

/// Find a specific provider specification by id or alias.
pub fn find_spec(id: &str) -> Option<ProviderSpec> {
    let lower = id.to_lowercase();
    load_catalog().into_iter().find(|p| p.id.to_lowercase() == lower)
}

/// Load user's saved providers configuration.
pub fn load_storage() -> ProvidersStorage {
    let path = providers_path();
    if let Ok(content) = fs::read_to_string(&path) {
        if let Ok(storage) = serde_json::from_str::<ProvidersStorage>(&content) {
            return storage;
        }
    }
    // Default initial configuration
    let mut storage = ProvidersStorage {
        active_provider: "custom_openai".to_string(),
        providers: HashMap::new(),
    };
    // Pre-populate custom_openai pointing to default loopback proxy
    storage.providers.insert(
        "custom_openai".to_string(),
        ProviderConfig {
            provider_id: "custom_openai".to_string(),
            base_url: Some("http://127.0.0.1:8317/v1".to_string()),
            api_key: Some("sk-cp-35209207c3e2541392cd9e28d1e402f7695a2e16842675bb".to_string()),
            oauth_token: None,
            active_model: Some("gpt-oss-120b-medium".to_string()),
            custom_headers: HashMap::new(),
        },
    );
    storage
}

/// Save user's providers configuration to disk.
pub fn save_storage(storage: &ProvidersStorage) -> Result<(), String> {
    let dir = onyx_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("create ~/.onyx dir: {e}"))?;
    let path = providers_path();
    let json = serde_json::to_string_pretty(storage).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Resolve effective API Key for a provider (from config -> env -> empty).
pub fn resolve_api_key(spec: &ProviderSpec, config: Option<&ProviderConfig>) -> Option<String> {
    if let Some(cfg) = config {
        if let Some(ref k) = cfg.api_key {
            if !k.trim().is_empty() {
                return Some(k.trim().to_string());
            }
        }
        if let Some(ref tok) = cfg.oauth_token {
            if !tok.trim().is_empty() {
                return Some(tok.trim().to_string());
            }
        }
    }
    if let Some(ref env_name) = spec.env_var {
        if let Ok(val) = std::env::var(env_name) {
            if !val.trim().is_empty() {
                return Some(val.trim().to_string());
            }
        }
    }
    None
}

/// Resolve effective Base URL for a provider (from config -> spec -> loopback).
pub fn resolve_base_url(spec: &ProviderSpec, config: Option<&ProviderConfig>) -> String {
    if let Some(cfg) = config {
        if let Some(ref u) = cfg.base_url {
            if !u.trim().is_empty() {
                return u.trim().trim_end_matches('/').to_string();
            }
        }
    }
    if let Some(ref u) = spec.base_url {
        if !u.trim().is_empty() {
            return u.trim().trim_end_matches('/').to_string();
        }
    }
    "http://127.0.0.1:8317/v1".to_string()
}

/// Structure for GitHub Copilot Device Flow response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

/// Start GitHub Copilot Device OAuth Flow.
pub fn start_copilot_device_flow() -> Result<DeviceCodeResponse, String> {
    let client_id = "Iv1.b507a08c87ecfe98";
    let body = serde_json::json!({
        "client_id": client_id,
        "scope": "read:user"
    });
    let raw = crate::model::http_request(
        "POST",
        "github.com",
        443,
        "/login/device/code",
        None,
        Some(&body.to_string()),
    );
    // If TLS fails via plain TcpStream, execute via curl fallback which is guaranteed on system
    let raw_resp = match raw {
        Ok(r) => r,
        Err(_) => {
            let output = std::process::Command::new("curl")
                .args([
                    "-s",
                    "-X", "POST",
                    "https://github.com/login/device/code",
                    "-H", "accept: application/json",
                    "-H", "content-type: application/json",
                    "-H", "editor-version: vscode/1.85.1",
                    "-H", "editor-plugin-version: copilot/1.155.0",
                    "-H", "user-agent: GithubCopilot/1.155.0",
                    "-d", &body.to_string(),
                ])
                .output()
                .map_err(|e| format!("execute curl device flow: {e}"))?;
            String::from_utf8_lossy(&output.stdout).to_string()
        }
    };

    serde_json::from_str::<DeviceCodeResponse>(&raw_resp)
        .map_err(|e| format!("parse device flow response: {e} ({raw_resp})"))
}

/// Poll GitHub Copilot Device OAuth token completion.
pub fn poll_copilot_device_token(device_code: &str) -> Result<Option<String>, String> {
    let client_id = "Iv1.b507a08c87ecfe98";
    let body = serde_json::json!({
        "client_id": client_id,
        "device_code": device_code,
        "grant_type": "urn:ietf:params:oauth:grant-type:device_code"
    });

    let output = std::process::Command::new("curl")
        .args([
            "-s",
            "-X", "POST",
            "https://github.com/login/oauth/access_token",
            "-H", "accept: application/json",
            "-H", "content-type: application/json",
            "-H", "editor-version: vscode/1.85.1",
            "-H", "editor-plugin-version: copilot/1.155.0",
            "-H", "user-agent: GithubCopilot/1.155.0",
            "-d", &body.to_string(),
        ])
        .output()
        .map_err(|e| format!("execute curl token poll: {e}"))?;

    let text = String::from_utf8_lossy(&output.stdout);
    if let Ok(val) = serde_json::from_str::<Value>(&text) {
        if let Some(tok) = val.get("access_token").and_then(|v| v.as_str()) {
            return Ok(Some(tok.to_string()));
        }
        if let Some(err) = val.get("error").and_then(|v| v.as_str()) {
            if err == "authorization_pending" || err == "slow_down" {
                return Ok(None);
            }
            return Err(format!("OAuth error: {err}"));
        }
    }
    Ok(None)
}
