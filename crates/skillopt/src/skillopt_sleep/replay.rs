// Auto-synthesized Rust implementation for skillopt_sleep/replay.py
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use async_trait::async_trait;

/* SkillOpt-Sleep — Stage 3: replay.

Re-run mined TaskRecords offline under a given (skill, memory) and score
them, producing the (hard, soft) signal SkillOpt's gate consumes.

Single-shot text replay by default. Tasks whose rule judge requires a tool
call (gbrain's `tool_called`) use the backend's tool-aware path. Backends that
cannot enforce that execution boundary fail explicitly rather than scoring a
self-reported tool call. */

pub fn _required_tools(task: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn aggregate_scores(pairs: serde_json::Value, ReplayResult]]: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn aggregate_cost(pairs: serde_json::Value, ReplayResult]]: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}
