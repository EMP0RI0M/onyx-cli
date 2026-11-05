// Auto-synthesized Rust implementation for skillopt_sleep/harvest_cursor.py
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use async_trait::async_trait;

/* Read Cursor Agent transcripts and normalize them into session digests.

Cursor writes workspace-scoped JSONL under
``~/.cursor/projects/<workspace>/agent-transcripts/<session>/<session>.jsonl``.
The observed local records contain user/assistant messages and tool-use metadata
but no timestamps, so this harvester uses each file's mtime as its end time.
Tool inputs and outputs are intentionally never copied. */

pub fn cursor_project_slug(project: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _text_from_content(content: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _tool_names(content: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _sanitize_text(text: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _dedup(values: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _mtime_iso(path: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _mtime(path: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _iso_epoch(value: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn digest_cursor_transcript(path: serde_json::Value, project: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _workspace_path(project_dir: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _normalized_path(path: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _is_workspace_ancestor(workspace: serde_json::Value, invoked: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _available_project_dirs(projects_dir: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _project_dirs(projects_dir: serde_json::Value, scope: serde_json::Value, invoked_project: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _is_cursor_replay(digest: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}
