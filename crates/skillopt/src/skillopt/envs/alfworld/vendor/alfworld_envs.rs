// Auto-synthesized Rust implementation for skillopt/envs/alfworld/vendor/alfworld_envs.py
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use async_trait::async_trait;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AlfworldWorker {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl AlfworldWorker {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct _ProcessWorker {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl _ProcessWorker {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AlfworldEnvs {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl AlfworldEnvs {
    pub fn new() -> Self {
        Self::default()
    }
}

pub fn load_config_file(path: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn compute_reward(info: serde_json::Value, multi_modal: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn step(action: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn reset() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _worker_loop(cmd_q: serde_json::Value, result_q: serde_json::Value, config: serde_json::Value, seed: serde_json::Value, is_train: serde_json::Value, eval_dataset: serde_json::Value, gamefile: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn send(cmd: serde_json::Value, payload: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn recv() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn close(kill: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn step(actions: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn reset() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn get_admissible_commands() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn close() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}
