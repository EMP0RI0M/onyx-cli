// Auto-synthesized Rust implementation for skillopt_sleep/harvest_opencode.py
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use async_trait::async_trait;

/* Read OpenCode session transcripts from its local SQLite database.

OpenCode normally stores sessions in ``opencode.db`` below its XDG data
directory.
This harvester opens that database read-only and keeps only visible user and
assistant text plus short tool names. Reasoning, tool arguments and results,
file payloads, patches, and provider metadata are never copied into digests. */

pub fn _opencode_data_candidates() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn default_opencode_db() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _ro_uri(path: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _open_database(path: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _table_columns(connection: serde_json::Value, table: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _has_supported_schema(connection: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _load_object(raw: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _sanitize_text(parts: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _sanitize_tool_name(value: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _tool_name(part: serde_json::Value, Any]: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _millis_to_iso(value: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _iso_to_millis(value: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _dedup(values: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _is_skillopt_session(row: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}
