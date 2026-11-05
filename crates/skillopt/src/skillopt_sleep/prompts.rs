// Auto-synthesized Rust implementation for skillopt_sleep/prompts.py
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use async_trait::async_trait;

/* SkillOpt-Sleep — central prompt registry with live user overrides.

Every LLM-facing prompt template the sleep cycle uses (miner / attempt /
judge / reflect) lives here, in one place, instead of being scattered as
inline literals. Two consequences:

  1. **Auditability** — the dashboard (and any human) can display exactly
     what instructions each agent role receives, per stage.
  2. **Live tuning** — a user override file (``prompts.json`` in the state
     dir, or ``SKILLOPT_SLEEP_PROMPTS_PATH``) replaces any template without
     touching code. The file's mtime is checked on every read, so an edit
     made while a cycle is running takes effect on the very next call.

Placeholders use the ``__NAME__`` convention (simple ``str.replace``, no
``str.format``) because the templates themselves contain JSON braces.

The default texts are byte-for-byte the prompts previously inlined in
``backend.py`` / ``llm_miner.py``, so behavior is unchanged unless the user
overrides a template. */

pub fn overrides_path() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn load_overrides() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn save_overrides(overrides: serde_json::Value, Optional[str]]: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn get_prompt(name: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn is_overridden(name: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn render(name: serde_json::Value, mapping: serde_json::Value, str]: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn describe() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}
