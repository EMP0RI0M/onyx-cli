// Auto-synthesized Rust implementation for skillopt/model/codex_harness.py
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use async_trait::async_trait;

/* Helpers for running exec backends as the target harness. */

pub fn _is_symlink_privilege_error(exc: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _build_codex_trace_summary(raw: serde_json::Value, response: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _find(prefix: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _build_claude_trace_summary(raw: serde_json::Value, response: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _build_cursor_trace_summary(raw: serde_json::Value, response: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _persist_codex_artifacts(work_dir: serde_json::Value, raw: serde_json::Value, response: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _persist_claude_artifacts(work_dir: serde_json::Value, raw: serde_json::Value, response: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _persist_cursor_artifacts(work_dir: serde_json::Value, raw: serde_json::Value, response: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn parse_codex_raw(raw: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn format_codex_trace_steps(raw: serde_json::Value, max_chars: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn extract_codex_trace_prefix(raw: serde_json::Value, after_step: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _claude_step_truncate(text: serde_json::Value, limit: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _summarize_claude_tool_call(name: serde_json::Value, input_data: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _iter_claude_json_blocks(raw: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn parse_claude_trace_steps(raw: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn format_claude_trace_steps(raw: serde_json::Value, max_chars: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _normalize_tools(allowed_tools: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _tools_list(allowed_tools: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _validate_exec_path(path: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _validated_add_dirs(work_dir: serde_json::Value, data_dirs: serde_json::Value, images: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _sdk_mode(value: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _claude_effort(value: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _json_default(obj: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _json_dumps(data: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _run_async(coro: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _target() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _exec_prompt(prompt: serde_json::Value, allow_file_edits: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _retry_prompt(prompt: serde_json::Value, attempt: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _normalize_target_exec_prompt(prompt: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _strict_schema(schema: serde_json::Value, Any]: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _structured_response(data: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _extract_claude_structured_output(messages: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _raw_exception(label: serde_json::Value, exc: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _query() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _claude_chat_text_from_messages(messages: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _claude_chat_usage_from_event(event: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _query() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _query() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _is_copilot_secret_key(field: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _find_json_end(text: serde_json::Value, start: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _redact_embedded_json(text: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _redact_cursor_error(value: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _sanitize_cursor_json(value: serde_json::Value, field: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _redact_copilot_json(value: serde_json::Value, field: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _redact_copilot_trace(raw: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _cursor_process_text(value: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _parse_cursor_terminal(raw: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}
