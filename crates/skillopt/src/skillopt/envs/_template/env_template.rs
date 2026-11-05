// Auto-synthesized Rust implementation for skillopt/envs/_template/env_template.py
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use async_trait::async_trait;

/* Benchmark Environment Template
===============================
Copy this file and implement the TODO sections to add a new benchmark.

The EnvAdapter is responsible for:
  1. Building per-batch environment managers (train and eval splits).
  2. Running rollouts under the current skill document.
  3. Reflecting on those rollouts into raw patch dicts.
  4. Reporting the distinct task types in your data (for stratified
     sampling).

For a fully worked example see ``skillopt/envs/officeqa/``. */

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TemplateBenchmarkEnv {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl TemplateBenchmarkEnv {
    pub fn new() -> Self {
        Self::default()
    }
}

pub fn setup(cfg: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn get_dataloader() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn build_env_from_batch(batch: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn build_train_env(batch_size: serde_json::Value, seed: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn build_eval_env(env_num: serde_json::Value, split: serde_json::Value, seed: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn get_task_types() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}
