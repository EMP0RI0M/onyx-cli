// Auto-generated zero-token config schema and flattening engine
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub env: Option<String>,
    #[serde(default)]
    pub out_root: Option<String>,
    #[serde(default)]
    pub skill_init: Option<String>,
    #[serde(default)]
    pub eval_test: Option<String>,
    #[serde(default)]
    pub gate_metric: Option<String>,
    #[serde(default)]
    pub gate_mixed_weight: Option<f64>,
    #[serde(default)]
    pub leading_words: Option<String>,
    #[serde(default)]
    pub sel_env_num: Option<String>,
    #[serde(default)]
    pub semantic_density_weight: Option<f64>,
    #[serde(default)]
    pub test_env_num: Option<String>,
    #[serde(default)]
    pub use_gate: Option<bool>,
    #[serde(default)]
    pub use_semantic_density: Option<bool>,
    #[serde(default)]
    pub analyst_workers: Option<String>,
    #[serde(default)]
    pub failure_only: Option<String>,
    #[serde(default)]
    pub merge_batch_size: Option<i64>,
    #[serde(default)]
    pub minibatch_size: Option<i64>,
    #[serde(default)]
    pub azure_api_key: Option<String>,
    #[serde(default)]
    pub azure_api_version: Option<String>,
    #[serde(default)]
    pub azure_endpoint: Option<String>,
    #[serde(default)]
    pub azure_openai_ad_scope: Option<String>,
    #[serde(default)]
    pub azure_openai_api_key: Option<String>,
    #[serde(default)]
    pub azure_openai_api_version: Option<String>,
    #[serde(default)]
    pub azure_openai_auth_mode: Option<String>,
    #[serde(default)]
    pub azure_openai_endpoint: Option<String>,
    #[serde(default)]
    pub azure_openai_managed_identity_client_id: Option<String>,
    #[serde(default)]
    pub model_backend: Option<String>,
    #[serde(default)]
    pub claude_code_exec_effort: Option<f64>,
    #[serde(default)]
    pub claude_code_exec_max_thinking_tokens: Option<i64>,
    #[serde(default)]
    pub claude_code_exec_path: Option<String>,
    #[serde(default)]
    pub claude_code_exec_profile: Option<String>,
    #[serde(default)]
    pub claude_code_exec_use_sdk: Option<bool>,
    #[serde(default)]
    pub claude_trace_to_optimizer: Option<bool>,
    #[serde(default)]
    pub codex_exec_approval_policy: Option<String>,
    #[serde(default)]
    pub codex_exec_full_auto: Option<bool>,
    #[serde(default)]
    pub codex_exec_network_access: Option<bool>,
    #[serde(default)]
    pub codex_exec_path: Option<String>,
    #[serde(default)]
    pub codex_exec_profile: Option<String>,
    #[serde(default)]
    pub codex_exec_reasoning_effort: Option<f64>,
    #[serde(default)]
    pub codex_exec_sandbox: Option<String>,
    #[serde(default)]
    pub codex_exec_use_sdk: Option<bool>,
    #[serde(default)]
    pub codex_exec_web_search: Option<bool>,
    #[serde(default)]
    pub codex_trace_to_optimizer: Option<bool>,
    #[serde(default)]
    pub copilot_chat_optimizer_model: Option<String>,
    #[serde(default)]
    pub copilot_chat_target_model: Option<String>,
    #[serde(default)]
    pub copilot_chat_timeout: Option<i64>,
    #[serde(default)]
    pub copilot_exec_allow_all_tools: Option<String>,
    #[serde(default)]
    pub copilot_exec_home: Option<String>,
    #[serde(default)]
    pub copilot_exec_path: Option<String>,
    #[serde(default)]
    pub cursor_exec_path: Option<String>,
    #[serde(default)]
    pub cursor_exec_sandbox: Option<String>,
    #[serde(default)]
    pub minimax_api_key: Option<String>,
    #[serde(default)]
    pub minimax_base_url: Option<String>,
    #[serde(default)]
    pub minimax_enable_thinking: Option<bool>,
    #[serde(default)]
    pub minimax_max_tokens: Option<i64>,
    #[serde(default)]
    pub minimax_model: Option<String>,
    #[serde(default)]
    pub minimax_region: Option<String>,
    #[serde(default)]
    pub minimax_temperature: Option<f64>,
    #[serde(default)]
    pub openai_compatible_api_key: Option<String>,
    #[serde(default)]
    pub openai_compatible_base_url: Option<String>,
    #[serde(default)]
    pub openai_compatible_max_tokens: Option<i64>,
    #[serde(default)]
    pub openai_compatible_model: Option<String>,
    #[serde(default)]
    pub openai_compatible_temperature: Option<f64>,
    #[serde(default)]
    pub openai_compatible_timeout_seconds: Option<i64>,
    #[serde(default)]
    pub optimizer_model: Option<String>,
    #[serde(default)]
    pub optimizer_azure_openai_ad_scope: Option<String>,
    #[serde(default)]
    pub optimizer_azure_openai_api_key: Option<String>,
    #[serde(default)]
    pub optimizer_azure_openai_api_version: Option<String>,
    #[serde(default)]
    pub optimizer_azure_openai_auth_mode: Option<String>,
    #[serde(default)]
    pub optimizer_azure_openai_endpoint: Option<String>,
    #[serde(default)]
    pub optimizer_azure_openai_managed_identity_client_id: Option<String>,
    #[serde(default)]
    pub optimizer_backend: Option<String>,
    #[serde(default)]
    pub optimizer_openai_compatible_api_key: Option<String>,
    #[serde(default)]
    pub optimizer_openai_compatible_base_url: Option<String>,
    #[serde(default)]
    pub optimizer_openai_compatible_max_tokens: Option<i64>,
    #[serde(default)]
    pub optimizer_openai_compatible_model: Option<String>,
    #[serde(default)]
    pub optimizer_openai_compatible_temperature: Option<f64>,
    #[serde(default)]
    pub optimizer_openai_compatible_timeout_seconds: Option<i64>,
    #[serde(default)]
    pub optimizer_qwen_chat_api_key: Option<String>,
    #[serde(default)]
    pub optimizer_qwen_chat_base_url: Option<String>,
    #[serde(default)]
    pub optimizer_qwen_chat_enable_thinking: Option<bool>,
    #[serde(default)]
    pub optimizer_qwen_chat_max_tokens: Option<i64>,
    #[serde(default)]
    pub optimizer_qwen_chat_temperature: Option<f64>,
    #[serde(default)]
    pub optimizer_qwen_chat_thinking_mode: Option<String>,
    #[serde(default)]
    pub optimizer_qwen_chat_timeout_seconds: Option<i64>,
    #[serde(default)]
    pub qwen_chat_api_key: Option<String>,
    #[serde(default)]
    pub qwen_chat_base_url: Option<String>,
    #[serde(default)]
    pub qwen_chat_enable_thinking: Option<bool>,
    #[serde(default)]
    pub qwen_chat_max_tokens: Option<i64>,
    #[serde(default)]
    pub qwen_chat_temperature: Option<f64>,
    #[serde(default)]
    pub qwen_chat_thinking_mode: Option<String>,
    #[serde(default)]
    pub qwen_chat_timeout_seconds: Option<i64>,
    #[serde(default)]
    pub reasoning_effort: Option<f64>,
    #[serde(default)]
    pub rewrite_max_completion_tokens: Option<i64>,
    #[serde(default)]
    pub rewrite_reasoning_effort: Option<f64>,
    #[serde(default)]
    pub target_model: Option<String>,
    #[serde(default)]
    pub target_azure_openai_ad_scope: Option<String>,
    #[serde(default)]
    pub target_azure_openai_api_key: Option<String>,
    #[serde(default)]
    pub target_azure_openai_api_version: Option<String>,
    #[serde(default)]
    pub target_azure_openai_auth_mode: Option<String>,
    #[serde(default)]
    pub target_azure_openai_endpoint: Option<String>,
    #[serde(default)]
    pub target_azure_openai_managed_identity_client_id: Option<String>,
    #[serde(default)]
    pub target_backend: Option<String>,
    #[serde(default)]
    pub target_openai_compatible_api_key: Option<String>,
    #[serde(default)]
    pub target_openai_compatible_base_url: Option<String>,
    #[serde(default)]
    pub target_openai_compatible_max_tokens: Option<i64>,
    #[serde(default)]
    pub target_openai_compatible_model: Option<String>,
    #[serde(default)]
    pub target_openai_compatible_temperature: Option<f64>,
    #[serde(default)]
    pub target_openai_compatible_timeout_seconds: Option<i64>,
    #[serde(default)]
    pub target_qwen_chat_api_key: Option<String>,
    #[serde(default)]
    pub target_qwen_chat_base_url: Option<String>,
    #[serde(default)]
    pub target_qwen_chat_enable_thinking: Option<bool>,
    #[serde(default)]
    pub target_qwen_chat_max_tokens: Option<i64>,
    #[serde(default)]
    pub target_qwen_chat_temperature: Option<f64>,
    #[serde(default)]
    pub target_qwen_chat_thinking_mode: Option<String>,
    #[serde(default)]
    pub target_qwen_chat_timeout_seconds: Option<i64>,
    #[serde(default)]
    pub edit_budget: Option<String>,
    #[serde(default)]
    pub longitudinal_pair_policy: Option<String>,
    #[serde(default)]
    pub lr_control_mode: Option<String>,
    #[serde(default)]
    pub lr_scheduler: Option<String>,
    #[serde(default)]
    pub meta_edit_budget: Option<String>,
    #[serde(default)]
    pub min_edit_budget: Option<String>,
    #[serde(default)]
    pub skill_aware_appendix_source: Option<String>,
    #[serde(default)]
    pub skill_aware_consolidate_threshold: Option<String>,
    #[serde(default)]
    pub skill_update_mode: Option<String>,
    #[serde(default)]
    pub slow_update_gate_with_selection: Option<String>,
    #[serde(default)]
    pub slow_update_samples: Option<String>,
    #[serde(default)]
    pub use_meta_skill: Option<bool>,
    #[serde(default)]
    pub use_skill_aware_reflection: Option<bool>,
    #[serde(default)]
    pub use_slow_update: Option<bool>,
    #[serde(default)]
    pub accumulation: Option<String>,
    #[serde(default)]
    pub batch_size: Option<i64>,
    #[serde(default)]
    pub num_epochs: Option<String>,
    #[serde(default)]
    pub seed: Option<String>,
    #[serde(default)]
    pub steps_per_epoch: Option<String>,
    #[serde(default)]
    pub train_size: Option<i64>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl Config {
    pub fn from_json_str(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}