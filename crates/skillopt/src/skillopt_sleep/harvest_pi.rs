// Auto-synthesized Rust implementation for skillopt_sleep/harvest_pi.py
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use async_trait::async_trait;

/* SkillOpt-Sleep — pi (pi-coding-agent) session harvesting.

Reads pi session transcript JSONL files (one per session, stored under
``~/.pi/agent/sessions/<project-slug>/<sessionId>.jsonl``) and normalizes them
into :class:`SessionDigest` records without copying tool arguments, private
reasoning blocks (``thinking``), or raw tool outputs.

pi schema (verified against the upstream session format):
  * A session file is a JSONL stream of entries with a ``type`` discriminator.
  * ``type == "session"``  — exactly one per file; carries ``cwd`` + ``timestamp``.
  * Version 1 sessions are linear. Versions 2 and 3 form an append-only tree
    using entry ``id`` / ``parentId`` fields; only the branch ending at the last
    entry is the active conversation and is harvested.
  * ``type == "message"``  — a conversational turn. ``message.role`` ∈
    {user, assistant, toolResult}; ``message.content`` is either a string or a
    list of content blocks. Block types include ``text`` (kept), ``thinking``
    (private reasoning, skipped), and ``toolCall`` (carries ``name``).
  * toolResult messages can carry ``isError`` and ``toolName``. Tool names are
    retained, while transient per-call errors are not treated as task feedback.
  * Other types (``model_change``, ``thinking_level_change``, ``custom``, ...) are
    metadata / tool-result payloads and are skipped for digestion.

This module performs NO writes and NO network calls. */

pub fn _redact_secrets(text: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _sanitize_tool_name(name: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _read_pi_jsonl(path: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _pi_tool_names_from_content(content: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _active_branch_entries(records: serde_json::Value, Any]]: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _dedup(xs: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn digest_pi_session(path: serde_json::Value, project: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}
