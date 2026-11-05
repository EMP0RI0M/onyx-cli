// Auto-synthesized Rust implementation for skillopt/envs/alfworld/dataloader.py
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use async_trait::async_trait;

/* ALFWorld task dataloader. */

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ALFWorldDataLoader {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl ALFWorldDataLoader {
    pub fn new() -> Self {
        Self::default()
    }
}

pub fn _metadata_for_items(items: serde_json::Value, split: serde_json::Value, phase: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn get_train_size() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn build_train_batch(batch_size: serde_json::Value, seed: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}
