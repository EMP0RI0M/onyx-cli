// Auto-synthesized Rust implementation for skillopt/model/copilot_backend.py
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use async_trait::async_trait;

/* GitHub Copilot CLI chat backend.

Drives the locally installed `copilot` CLI as a chat model so a full SkillOpt
run (optimizer *and* target) can execute with no separate provider API key.
Inference still uses the GitHub Copilot cloud service through the operator's
existing subscription and OS credential store.

The CLI is a single-shot agent, not a chat-completions endpoint, so `system`
and `user` are composed into one prompt. Output is captured as JSONL
(`--output-format json`) and the `assistant.message` content is concatenated;
the plain-text/`--silent` modes do not reliably stream to stdout on all
platforms.

The CLI does not report token usage, so usage counters are reported as zeros.
Cost/quota is governed by the Copilot subscription rather than per-token
billing, so this does not lose billable accounting. */

pub fn build_copilot_subprocess_env(home: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _compose_prompt(system: serde_json::Value, user: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _messages_to_prompt(messages: serde_json::Value, Any]]: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn parse_copilot_jsonl(raw: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _invoke(prompt: serde_json::Value, model: serde_json::Value, timeout: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn get_token_summary() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn reset_token_tracker() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}
