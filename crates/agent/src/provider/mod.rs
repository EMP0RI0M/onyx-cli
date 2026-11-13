//! Multi-Provider Registry, Configuration & Authentication subsystem.
//! Powered by catalog definitions from liter-llm (174+ LLM providers, OAuth Device
//! flow, API keys, and custom endpoints). Protocol adapters (OpenAI-compat,
//! Anthropic Messages, Google Gemini) live in sibling modules and are selected per
//! provider by `registry::protocol_for`.

pub mod anthropic;
pub mod cliproxy;
pub mod gemini;
pub mod openai;
pub mod registry;
pub mod transport;
pub mod types;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

pub use registry::{protocol_for, resolve, Resolved};
pub use types::{Header, ParsedReply, ProviderProtocol, RequestCtx, StreamAcc, Usage};

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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderConfig {
    pub provider_id: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub oauth_token: Option<String>,
    #[serde(default)]
    pub oauth_refresh_token: Option<String>,
    #[serde(default)]
    pub oauth_expires_at: Option<u64>,
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
    // Pre-populate custom_openai pointing to the default loopback router (OmniRoute).
    // No key is baked in: a loopback router accepts none, and any real secret comes
    // from the credential vault or an env var at resolve time (see `resolve_api_key`).
    storage.providers.insert(
        "custom_openai".to_string(),
        ProviderConfig {
            provider_id: "custom_openai".to_string(),
            base_url: Some("http://127.0.0.1:20128/v1".to_string()),
            api_key: None,
            oauth_token: None,
            oauth_refresh_token: None,
            oauth_expires_at: None,
            active_model: Some("gpt-oss-120b-medium".to_string()),
            custom_headers: HashMap::new(),
        },
    );
    storage
}

/// Save user's providers configuration to disk. Secrets are NOT written here when
/// the vault is available — `save_storage` strips inline keys into the vault and
/// leaves only a `vault:<slot>` reference in providers.json, so the plaintext
/// config never holds a live credential. Mode 0600 on the file regardless.
pub fn save_storage(storage: &ProvidersStorage) -> Result<(), String> {
    let dir = onyx_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("create ~/.onyx dir: {e}"))?;
    let path = providers_path();
    let json = serde_json::to_string_pretty(storage).map_err(|e| format!("serialize: {e}"))?;
    write_secret_file(&path, &json)
}

/// Write a file with mode 0600 before contents land (config may still hold refs).
fn write_secret_file(path: &std::path::Path, contents: &str) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    std::io::Write::write_all(&mut f, contents.as_bytes()).map_err(|e| format!("write: {e}"))?;
    Ok(())
}

/// Resolve effective API Key for a provider. Precedence: env var (always wins, so
/// an exported key overrides stored state) → vault reference (`vault:<slot>` in the
/// config) → inline config key/OAuth token → vault slot named `<provider>/api_key`.
/// The credential vault is consulted through `onyx_core::Vault` when reachable; if a
/// `vault:` reference can't be resolved we treat it as absent rather than erroring.
pub fn resolve_api_key(spec: &ProviderSpec, config: Option<&ProviderConfig>) -> Option<String> {
    // 1. Environment override first.
    if spec.id == "cliproxyapi" {
        if let Ok(val) = std::env::var("CLIPROXYAPI_API_KEY") {
            if !val.trim().is_empty() {
                return Some(val.trim().to_string());
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
    if let Some(cfg) = config {
        // 2. A vault reference stored instead of the literal secret.
        if let Some(slot) = cfg.api_key.as_deref().and_then(vault_slot_of) {
            if let Some(v) = vault_lookup(&slot) {
                return Some(v);
            }
        }
        // 3. Inline key (legacy configs / explicit set).
        if let Some(ref k) = cfg.api_key {
            if !k.trim().is_empty() && vault_slot_of(k).is_none() {
                return Some(k.trim().to_string());
            }
        }
        if let Some(ref tok) = cfg.oauth_token {
            if let Some(slot) = vault_slot_of(tok) {
                if let Some(v) = vault_lookup(&slot) {
                    return Some(v);
                }
            } else if !tok.trim().is_empty() {
                return Some(tok.trim().to_string());
            }
        }
    }
    // 4. Implicit vault slot keyed by provider id.
    if let Some(v) = vault_lookup(&format!("{}/api_key", spec.id)) {
        return Some(v);
    }
    None
}

/// Recognise a `vault:<slot>` indirection marker stored in providers.json.
fn vault_slot_of(s: &str) -> Option<String> {
    s.trim().strip_prefix("vault:").map(|slot| slot.to_string())
}

/// Look up one slot in the on-disk credential vault. Best-effort: any failure to
/// open/decrypt the vault yields None (the caller then falls back to other sources).
/// An optional passphrase from `ONYX_VAULT_PASSPHRASE` selects the encrypted tier.
fn vault_lookup(slot: &str) -> Option<String> {
    let path = crate::provider::vault_path();
    let pass = std::env::var("ONYX_VAULT_PASSPHRASE").ok().filter(|p| !p.is_empty());
    let vault = onyx_core::Vault::load(path, pass).ok()?;
    vault.resolve(slot)
}

/// Store one secret into the vault and return the `vault:<slot>` marker to persist
/// in providers.json in place of the literal. Falls back to returning the raw value
/// if the vault cannot be written (so callers always have something to store).
pub fn put_secret(slot: &str, value: &str) -> String {
    let path = crate::provider::vault_path();
    let pass = std::env::var("ONYX_VAULT_PASSPHRASE").ok().filter(|p| !p.is_empty());
    match onyx_core::Vault::load(path.clone(), pass.clone()) {
        Ok(mut vault) => {
            vault.set(slot, value);
            if vault.save().is_ok() {
                return format!("vault:{slot}");
            }
            value.to_string()
        }
        Err(_) => value.to_string(),
    }
}

/// Path used for the credential vault (kept next to providers.json under ~/.onyx).
pub fn vault_path() -> PathBuf {
    onyx_dir().join("credentials.json")
}

/// Resolve effective Base URL for a provider (from config -> env -> spec -> loopback).
pub fn resolve_base_url(spec: &ProviderSpec, config: Option<&ProviderConfig>) -> String {
    if spec.id == "cliproxyapi" {
        if let Ok(val) = std::env::var("CLIPROXYAPI_BASE_URL") {
            if !val.trim().is_empty() {
                return val.trim().trim_end_matches('/').to_string();
            }
        }
    }
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
    "http://127.0.0.1:20128/v1".to_string()
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

/// Structure for Copilot internal session token exchange
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopilotSessionToken {
    pub token: String,
    pub expires_at: u64,
    pub endpoints: Option<HashMap<String, String>>,
}

/// Exchange GitHub access token for Copilot internal session key and endpoint
pub fn exchange_copilot_session_token(github_token: &str) -> Result<CopilotSessionToken, String> {
    let output = std::process::Command::new("curl")
        .args([
            "-s",
            "-X", "GET",
            "https://api.github.com/copilot_internal/v2/token",
            "-H", "accept: application/json",
            "-H", "editor-version: vscode/1.85.1",
            "-H", "editor-plugin-version: copilot/1.155.0",
            "-H", "user-agent: GithubCopilot/1.155.0",
            "-H", &format!("authorization: token {github_token}"),
        ])
        .output()
        .map_err(|e| format!("execute curl copilot token exchange: {e}"))?;

    let text = String::from_utf8_lossy(&output.stdout);
    if let Ok(val) = serde_json::from_str::<Value>(&text) {
        if let Some(tok) = val.get("token").and_then(|v| v.as_str()) {
            let expires_at = val.get("expires_at").and_then(|v| v.as_u64()).unwrap_or(0);
            let endpoints = val.get("endpoints").and_then(|e| {
                serde_json::from_value::<HashMap<String, String>>(e.clone()).ok()
            });
            return Ok(CopilotSessionToken {
                token: tok.to_string(),
                expires_at,
                endpoints,
            });
        }
        if let Some(err) = val.get("message").and_then(|v| v.as_str()) {
            return Err(format!("Copilot token exchange failed: {err}"));
        }
    }
    Err(format!("invalid copilot token exchange response: {text}"))
}

/// Structure for Google Vertex / Service Account OAuth
pub fn exchange_google_service_account_token(_key_json_path: &str) -> Result<String, String> {
    // If gcloud is installed, run `gcloud auth print-access-token`
    if let Ok(output) = std::process::Command::new("gcloud")
        .args(["auth", "print-access-token"])
        .output()
    {
        if output.status.success() {
            let tok = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !tok.is_empty() {
                return Ok(tok);
            }
        }
    }
    Err("gcloud CLI not authenticated. Run `gcloud auth application-default login` or set GOOGLE_APPLICATION_CREDENTIALS / GEMINI_API_KEY".to_string())
}

/// Provider health diagnostic result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthStatus {
    Ok(String),
    ConnectionRefused(String),
    AuthFailure(String),
    Timeout(String),
    Error(String),
}

/// Redact sensitive API keys or OAuth credentials from strings for safe diagnostics and logs.
pub fn redact_secret(secret: &str) -> String {
    let trimmed = secret.trim();
    if trimmed.is_empty() {
        return "(none)".to_string();
    }
    if trimmed.len() <= 8 {
        return "***".to_string();
    }
    let prefix = &trimmed[..4];
    let suffix = &trimmed[trimmed.len() - 4..];
    format!("{prefix}...{suffix}")
}

/// Check the health of a provider endpoint without leaking credentials.
pub fn check_provider_health(spec: &ProviderSpec, config: Option<&ProviderConfig>) -> HealthStatus {
    let base_url = resolve_base_url(spec, config);
    let key = resolve_api_key(spec, config);

    let (host, port, _sub) = crate::provider::registry::split_base(&base_url);
    let timeout = std::time::Duration::from_secs(3);

    // 1. TCP connection test
    let socket_addr = format!("{host}:{port}");
    let stream = match std::net::TcpStream::connect_timeout(
        &socket_addr
            .parse()
            .unwrap_or_else(|_| std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)), port)),
        timeout,
    ) {
        Ok(s) => s,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::ConnectionRefused {
                let hint = if spec.id == "cliproxyapi" {
                    format!("CLIProxyAPI is not running on {socket_addr}. Start CLIProxyAPI-Rust (`cliproxyapi-rust` or via Docker on port {port}) before connecting.")
                } else {
                    format!("connection refused on {socket_addr}")
                };
                return HealthStatus::ConnectionRefused(hint);
            } else if e.kind() == std::io::ErrorKind::TimedOut {
                return HealthStatus::Timeout(format!("timed out connecting to {socket_addr}"));
            } else {
                return HealthStatus::Error(format!("network error connecting to {socket_addr}: {e}"));
            }
        }
    };
    drop(stream);

    // 2. HTTP probe to /models endpoint
    let models_path = if base_url.ends_with("/v1") {
        "/v1/models"
    } else {
        "/models"
    };

    let auth_hdr = key.as_deref();
    match crate::model::http_request("GET", &host, port, models_path, auth_hdr, None) {
        Ok(body) => {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                if let Some(err) = v.get("error") {
                    let code = err.get("code").and_then(|c| c.as_u64()).unwrap_or(0);
                    let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error");
                    if code == 401 || msg.to_lowercase().contains("unauthorized") || msg.to_lowercase().contains("auth") {
                        return HealthStatus::AuthFailure(format!("authentication failed (HTTP 401): {msg}"));
                    }
                    return HealthStatus::Error(format!("provider error: {msg}"));
                }
                if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
                    return HealthStatus::Ok(format!("reachable on {socket_addr} · {} models discovered", arr.len()));
                }
            }
            HealthStatus::Ok(format!("reachable on {socket_addr}"))
        }
        Err(e) => {
            let lower = e.to_lowercase();
            if lower.contains("401") || lower.contains("unauthorized") {
                HealthStatus::AuthFailure("authentication failed (HTTP 401)".to_string())
            } else if lower.contains("timed out") || lower.contains("timeout") {
                HealthStatus::Timeout(format!("HTTP probe to {socket_addr} timed out"))
            } else if lower.contains("refused") {
                HealthStatus::ConnectionRefused(format!("connection refused on {socket_addr}"))
            } else {
                HealthStatus::Error(format!("health check probe error: {e}"))
            }
        }
    }
}

