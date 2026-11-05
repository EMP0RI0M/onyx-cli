// Auto-synthesized Rust implementation for skillopt_sleep/harvest_copilot_cli.py
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use async_trait::async_trait;

/* SkillOpt-Sleep GitHub Copilot CLI session harvesting.

The Copilot CLI keeps a global SQLite index at ``~/.copilot/session-store.db``
that already carries everything a :class:`SessionDigest` needs: per-session
``cwd``/``branch`` and per-turn user/assistant text. Reading it is far cheaper
than replaying the multi-gigabyte per-session ``events.jsonl`` logs, and it is
the same data the CLI itself exposes.

The store is written by live CLI sessions, so all reads go through a read-only
connection and fall back to a private snapshot when the live WAL cannot be
opened read-only. A harvest must never block or corrupt an in-flight session. */

pub fn default_session_store() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _clip(text: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _ro_uri(path: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _norm_ts(value: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _connect(store_path: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}
