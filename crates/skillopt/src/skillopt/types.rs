use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Allowed operations on a skill document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditOp {
    Append,
    InsertAfter,
    Replace,
    Delete,
}

impl Default for EditOp {
    fn default() -> Self {
        Self::Append
    }
}

/// Source type for an edit or reflection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    Failure,
    Success,
}

/// A single edit operation on a skill document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edit {
    pub op: EditOp,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_type: Option<SourceType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_level: Option<i64>,
    #[serde(default)]
    pub update_origin: String,
    #[serde(default)]
    pub update_target: String,
}

impl Default for Edit {
    fn default() -> Self {
        Self {
            op: EditOp::Append,
            content: String::new(),
            target: String::new(),
            support_count: None,
            source_type: None,
            merge_level: None,
            update_origin: String::new(),
            update_target: String::new(),
        }
    }
}

/// A set of edits with reasoning.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Patch {
    #[serde(default)]
    pub edits: Vec<Edit>,
    #[serde(default)]
    pub reasoning: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranking_details: Option<serde_json::Value>,
}

/// Result of a single episode/task rollout.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RolloutResult {
    pub id: String,
    pub hard: f64,
    pub soft: f64,
    #[serde(default)]
    pub n_turns: i64,
    #[serde(default)]
    pub fail_reason: String,
    #[serde(default)]
    pub task_type: String,
    #[serde(default)]
    pub task_description: String,
    #[serde(default)]
    pub predicted_answer: String,
    #[serde(default)]
    pub question: String,
    #[serde(default)]
    pub reference_text: String,
    #[serde(default)]
    pub target_system_prompt: String,
    #[serde(default)]
    pub target_user_prompt: String,
    #[serde(default)]
    pub spreadsheet_preview: String,
    #[serde(flatten)]
    pub extras: HashMap<String, serde_json::Value>,
}

/// One entry in the failure summary produced by error analysts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FailureSummaryEntry {
    pub failure_type: String,
    #[serde(default)]
    pub count: i64,
    #[serde(default)]
    pub description: String,
}

/// Analyst output from the Reflect stage — a patch with provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawPatch {
    pub patch: Patch,
    #[serde(default = "default_source_type_failure")]
    pub source_type: SourceType,
    #[serde(default)]
    pub batch_size: i64,
    #[serde(default)]
    pub failure_summary: Vec<FailureSummaryEntry>,
}

fn default_source_type_failure() -> SourceType {
    SourceType::Failure
}

/// Output of the epoch-level slow update stage (EMA / regularization).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SlowUpdateResult {
    #[serde(default)]
    pub reasoning: String,
    #[serde(default)]
    pub slow_update_content: String,
    #[serde(default)]
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_hard: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curr_hard: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_hard: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_soft: Option<f64>,
    #[serde(default)]
    pub candidate_hash: String,
    #[serde(default)]
    pub update_origin: String,
    #[serde(default)]
    pub update_target: String,
}

/// Validation gate decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateAction {
    AcceptNewBest,
    Accept,
    Reject,
}

/// Immutable outcome of the validation gate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateResult {
    pub action: GateAction,
    pub current_skill: String,
    pub current_score: f64,
    pub best_skill: String,
    pub best_score: f64,
    pub best_step: i64,
}

/// A concrete batch request consumed by the training loop.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BatchSpec {
    pub phase: String,
    pub split: String,
    pub seed: u64,
    pub batch_size: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}
