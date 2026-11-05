// Auto-synthesized Rust implementation for skillopt_sleep/config.py
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use async_trait::async_trait;

/* SkillOpt-Sleep — configuration.

Config is JSON-first (yaml optional) so the engine and the deterministic
experiment run with zero external dependencies. Defaults are safe:
review-gated adoption, single-project scope, bounded token/task budgets.

Resolution order (later wins):
  1. built-in DEFAULTS
  2. ~/.skillopt-sleep/config.json  (or .yaml if PyYAML available)
  3. explicit overrides passed to load_config(**overrides) */

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SleepConfig {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl SleepConfig {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigError {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl ConfigError {
    pub fn new() -> Self {
        Self::default()
    }
}

pub fn get(key: serde_json::Value, default: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn to_dict() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn state_dir() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn state_path() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn transcripts_dir() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn codex_archived_sessions_dir() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn pi_sessions_dir() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn cursor_projects_dir() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn copilot_cli_session_store() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn opencode_db_path() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn vscode_workspace_storage() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn history_path() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn skills_dir() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn managed_skill_path() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _user_config_path() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _load_file(path: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn load_config() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}
