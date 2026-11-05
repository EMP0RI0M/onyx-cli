// Auto-synthesized Rust implementation for skillopt/envs/spreadsheetbench/dataloader.py
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use async_trait::async_trait;

/* SpreadsheetBench task dataloader. */

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpreadsheetBenchDataLoader {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl SpreadsheetBenchDataLoader {
    pub fn new() -> Self {
        Self::default()
    }
}
