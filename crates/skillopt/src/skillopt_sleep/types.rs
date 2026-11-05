use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Stage 1: Normalized summary of one local agent session transcript.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionDigest {
    pub session_id: String,
    pub project: String,
    #[serde(default)]
    pub git_branch: String,
    #[serde(default)]
    pub started_at: String,
    #[serde(default)]
    pub ended_at: String,
    #[serde(default)]
    pub user_prompts: Vec<String>,
    #[serde(default)]
    pub assistant_finals: Vec<String>,
    #[serde(default)]
    pub tools_used: Vec<String>,
    #[serde(default)]
    pub skills_used: Vec<String>,
    #[serde(default)]
    pub files_touched: Vec<String>,
    #[serde(default)]
    pub feedback_signals: Vec<String>,
    #[serde(default)]
    pub n_user_turns: usize,
    #[serde(default)]
    pub n_assistant_turns: usize,
    #[serde(default)]
    pub raw_path: String,
}

/// Stage 2: Mined recurring task record.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: String,
    pub project: String,
    pub intent: String,
    #[serde(default)]
    pub context_excerpt: String,
    #[serde(default)]
    pub system: String,
    #[serde(default)]
    pub attempted_solution: String,
    #[serde(default = "default_unknown")]
    pub outcome: String,
    #[serde(default = "default_none")]
    pub reference_kind: String,
    #[serde(default)]
    pub reference: String,
    #[serde(default)]
    pub judge: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub source_sessions: Vec<String>,
    #[serde(default = "default_train")]
    pub split: String,
    #[serde(default = "default_real")]
    pub origin: String,
    #[serde(default)]
    pub derived_from: String,
    #[serde(default)]
    pub skill_hint: String,
}

fn default_unknown() -> String { "unknown".to_string() }
fn default_none() -> String { "none".to_string() }
fn default_train() -> String { "train".to_string() }
fn default_real() -> String { "real".to_string() }

/// Stage 3: Outcome of replaying one task record.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReplayResult {
    pub id: String,
    pub hard: f64,
    pub soft: f64,
    #[serde(default)]
    pub response: String,
    #[serde(default)]
    pub fail_reason: String,
    #[serde(default = "default_task")]
    pub task_type: String,
    #[serde(default)]
    pub judge_rationale: String,
    #[serde(default)]
    pub tools_called: Vec<String>,
    #[serde(default)]
    pub tokens: usize,
    #[serde(default)]
    pub latency_ms: f64,
    #[serde(default)]
    pub optimizer_feedback: String,
}

fn default_task() -> String { "task".to_string() }

/// Stage 4: Single bounded edit proposed/applied to skill or memory.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EditRecord {
    pub target: String,
    pub op: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub anchor: String,
    #[serde(default)]
    pub rationale: String,
}

/// Skill Group Report for sleep cycle gating.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SkillGroupReport {
    pub skill_name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub accepted: bool,
    #[serde(default)]
    pub gate_action: String,
    #[serde(default)]
    pub baseline_score: f64,
    #[serde(default)]
    pub candidate_score: f64,
    #[serde(default)]
    pub n_tasks: usize,
    #[serde(default)]
    pub n_applied_edits: usize,
    #[serde(default)]
    pub n_rejected_edits: usize,
    #[serde(default)]
    pub reason: String,
}
