// Auto-synthesized Rust implementation for skillopt_sleep/backend.py
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use async_trait::async_trait;

/* SkillOpt-Sleep — optimizer/replay backend abstraction.

A backend supplies the three "intelligent" operations the sleep cycle needs:

  1. attempt(task, skill, memory)  -> response text          (the rollout)
  2. judge(task, response)         -> (hard, soft, rationale) (the reward)
  3. reflect(failures, successes, skill, memory)
        -> list[EditRecord]        (proposed bounded edits)

Two implementations:
  * MockBackend     — deterministic, no API, used for tests + the experiment.
                      Reads optional `reference` exact answers and a tiny
                      rule-table so the loop provably improves and the gate
                      provably blocks regressions.
  * AnthropicBackend — uses the user's ANTHROPIC_API_KEY via the `claude`
                       CLI or the anthropic SDK (lazy-imported). Real lift.

The backend never touches live config; it only returns text/edits that the
consolidation stage gates and stages. */

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Backend {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl Backend {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MockBackend {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl MockBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CliBackend {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl CliBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PiCliBackend {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl PiCliBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClaudeCliBackend {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl ClaudeCliBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenCodeError {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl OpenCodeError {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct _OpenCodeReplayProject {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl _OpenCodeReplayProject {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenCodeCliBackend {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl OpenCodeCliBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodexCliBackend {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl CodexCliBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CursorBackendError {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl CursorBackendError {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CopilotCliBackend {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl CopilotCliBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CursorCliBackend {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl CursorCliBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DualBackend {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl DualBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AzureOpenAIBackend {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl AzureOpenAIBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AzureResponsesBackend {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl AzureResponsesBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

pub fn skill_hash(content: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn judge(task: serde_json::Value, response: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn tokens_used() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _normalize(s: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn exact_score(reference: serde_json::Value, response: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn keyword_soft_score(reference: serde_json::Value, response: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _optimizer_feedback(task: serde_json::Value, result: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _required_rules(task: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn attempt_with_tools(task: serde_json::Value, skill: serde_json::Value, memory: serde_json::Value, tools: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn judge(task: serde_json::Value, response: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _extract_json(raw: serde_json::Value, kind: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _task_guardrail(pairs: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _call(prompt: serde_json::Value, max_tokens: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _record_delta(delta: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _record_cost(prompt: serde_json::Value, response: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _reset_call_delta() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _cached_call(key: serde_json::Value, prompt: serde_json::Value, max_tokens: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn judge(task: serde_json::Value, response: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _cache_get(key: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _cache_pop_if(key: serde_json::Value, expected: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn tokens_used() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn token_delta() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _stream_text(value: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _set_call_error(message: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _cached_call(key: serde_json::Value, prompt: serde_json::Value, max_tokens: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _call(prompt: serde_json::Value, max_tokens: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _detect_cli_error(stdout: serde_json::Value, stderr: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _call(prompt: serde_json::Value, max_tokens: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn attempt_with_tools(task: serde_json::Value, skill: serde_json::Value, memory: serde_json::Value, tools: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn resolve_opencode_path(explicit: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _parse_opencode_jsonl_events(raw: serde_json::Value, expected_tool_ids: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _write_exclusive_text(path: serde_json::Value, content: serde_json::Value, encoding: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _normalize_opencode_tool_names(tools: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _prepare_opencode_replay_project(work: serde_json::Value, tools: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _opencode_temporary_workspace(prefix: serde_json::Value, error: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _build_child_environment(work: serde_json::Value, permission: serde_json::Value, str]: serde_json::Value, tools: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _build_run_command(work: serde_json::Value, agent: serde_json::Value, title: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _read_mcp_config(env: serde_json::Value, str]: serde_json::Value, work: serde_json::Value, stage: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _disable_and_verify_mcp(env: serde_json::Value, str]: serde_json::Value, work: serde_json::Value, config: serde_json::Value, Any]: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _verify_tool_allowlist(env: serde_json::Value, str]: serde_json::Value, work: serde_json::Value, agent: serde_json::Value, expected: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _cached_call(key: serde_json::Value, prompt: serde_json::Value, max_tokens: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _call(prompt: serde_json::Value, max_tokens: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn resolve_codex_path(explicit: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _call_once(prompt: serde_json::Value, max_tokens: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _call(prompt: serde_json::Value, max_tokens: serde_json::Value, retries: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn attempt_with_tools(task: serde_json::Value, skill: serde_json::Value, memory: serde_json::Value, tools: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn resolve_copilot_path(explicit: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn resolve_cursor_path(explicit: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _call(prompt: serde_json::Value, max_tokens: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _parse_jsonl_response(raw: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn attempt_with_tools(task: serde_json::Value, skill: serde_json::Value, memory: serde_json::Value, tools: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _command(workspace: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _terminal_result(raw: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _parse_json_response(raw: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _error(message: serde_json::Value, retryable: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _isolated_environment(runtime_dir: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _invoke_once(prompt: serde_json::Value, workspace: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _call(prompt: serde_json::Value, max_tokens: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn attempt(task: serde_json::Value, skill: serde_json::Value, memory: serde_json::Value, sample_id: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn attempt_with_tools(task: serde_json::Value, skill: serde_json::Value, memory: serde_json::Value, tools: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn judge(task: serde_json::Value, response: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn reflect(failures: serde_json::Value, successes: serde_json::Value, skill: serde_json::Value, memory: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _call(prompt: serde_json::Value, max_tokens: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn tokens_used() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn token_delta() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _endpoint_for(deployment: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _compat_mode() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _read_compat_max_tokens() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _read_chat_extra_body() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _usage_tokens(usage: serde_json::Value, input_field: serde_json::Value, output_field: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _as_int(value: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _is_azure_host() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _get_client() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _call(prompt: serde_json::Value, max_tokens: serde_json::Value, retries: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _get_provider() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _client_for(endpoint: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _next_endpoint() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _call(prompt: serde_json::Value, max_tokens: serde_json::Value, retries: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}
