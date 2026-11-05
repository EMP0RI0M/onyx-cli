// Auto-synthesized Rust implementation for skillopt/envs/_template/loader_template.py
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use async_trait::async_trait;

/* Benchmark Data Loader Template
================================
Copy this file and implement ``load_split_items`` to load your benchmark
data. The loader is a :class:`skillopt.datasets.base.SplitDataLoader`
subclass — the base class handles both ``split_mode="split_dir"`` (read
an existing train/val/test layout) and ``split_mode="ratio"`` (build the
splits from a single raw file deterministically).

For a fully worked example see
``skillopt/envs/officeqa/dataloader.py``. */

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TemplateBenchmarkLoader {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl TemplateBenchmarkLoader {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct note {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl note {
    pub fn new() -> Self {
        Self::default()
    }
}

pub fn _normalize_item(raw: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn load_split_items(split_path: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn load_raw_items(data_path: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}
