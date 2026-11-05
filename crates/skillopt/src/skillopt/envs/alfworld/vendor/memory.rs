// Auto-synthesized Rust implementation for skillopt/envs/alfworld/vendor/memory.py
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use async_trait::async_trait;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BaseMemory {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl BaseMemory {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SimpleMemory {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl SimpleMemory {
    pub fn new() -> Self {
        Self::default()
    }
}

pub fn reset(batch_size: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn store(record: serde_json::Value, List[Any]]: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn fetch(step: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn reset(batch_size: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn store(record: serde_json::Value, List[Any]]: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}
