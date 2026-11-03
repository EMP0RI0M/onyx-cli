//! update_goal tool — model-driven goal tracking, milestones, and status transitions.
use crate::{Args, Output, Tool, ToolError};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    NotStarted,
    InProgress,
    Completed,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressEntry {
    pub message: String,
    pub timestamp_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalState {
    pub goal: String,
    pub status: GoalStatus,
    pub progress: Vec<ProgressEntry>,
    pub blocked_reason: Option<String>,
    pub completion_summary: Option<String>,
}

impl Default for GoalState {
    fn default() -> Self {
        Self {
            goal: String::new(),
            status: GoalStatus::NotStarted,
            progress: Vec::new(),
            blocked_reason: None,
            completion_summary: None,
        }
    }
}

#[derive(Clone, Default)]
pub struct UpdateGoalTool {
    state: Arc<Mutex<GoalState>>,
}

impl UpdateGoalTool {
    pub fn new(state: Arc<Mutex<GoalState>>) -> Self {
        Self { state }
    }

    pub fn state(&self) -> Arc<Mutex<GoalState>> {
        Arc::clone(&self.state)
    }
}

impl Tool for UpdateGoalTool {
    fn name(&self) -> &str {
        "update_goal"
    }

    fn description(&self) -> &str {
        "Model-driven goal progress reporting. Set active goal, log progress milestones, mark completed: true when done, or blocked_reason if stuck. Args: goal (optional string), message (optional string), completed (optional bool), blocked_reason (optional string)."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec![
            "goal".into(),
            "message".into(),
            "completed".into(),
            "blocked_reason".into(),
        ]
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let mut st = self.state.lock().map_err(|e| ToolError::Other(e.to_string()))?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // 1. Goal update
        if let Some(g) = args.get("goal").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
            st.goal = g.to_string();
            if st.status == GoalStatus::NotStarted {
                st.status = GoalStatus::InProgress;
            }
        }

        // 2. Message / Progress log
        let msg_opt = args.get("message")
            .or_else(|| args.get("progress"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());

        if let Some(msg) = msg_opt {
            st.progress.push(ProgressEntry {
                message: msg.to_string(),
                timestamp_secs: now,
            });
        }

        // 3. Blocked reason check
        if let Some(reason) = args.get("blocked_reason").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
            st.status = GoalStatus::Blocked;
            st.blocked_reason = Some(reason.to_string());
        }

        // 4. Completed flag
        if let Some(completed) = args.get("completed").and_then(|v| v.as_bool()) {
            if completed {
                st.status = GoalStatus::Completed;
                st.blocked_reason = None;
                if let Some(msg) = msg_opt {
                    st.completion_summary = Some(msg.to_string());
                }
            } else if st.status == GoalStatus::Completed {
                st.status = GoalStatus::InProgress;
                st.completion_summary = None;
            }
        }

        let status_str = match st.status {
            GoalStatus::NotStarted => "not_started",
            GoalStatus::InProgress => "in_progress",
            GoalStatus::Completed => "completed",
            GoalStatus::Blocked => "blocked",
        };

        let active_goal = if st.goal.is_empty() { "(none set)" } else { &st.goal };
        let mut response = format!("Goal: \"{active_goal}\"\nStatus: {status_str}\nMilestones logged: {}", st.progress.len());
        if let Some(reason) = &st.blocked_reason {
            response.push_str(&format!("\nBlocked reason: {reason}"));
        }
        if let Some(summary) = &st.completion_summary {
            response.push_str(&format!("\nCompletion summary: {summary}"));
        }

        Ok(Output::ok(response))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn update_goal_transitions() {
        let tool = UpdateGoalTool::default();
        let res1 = tool.execute(&json!({"goal": "Ship Grok Replica", "message": "Planning tools"})).unwrap();
        assert!(res1.content.contains("in_progress"));
        assert!(res1.content.contains("Milestones logged: 1"));

        let res2 = tool.execute(&json!({"completed": true, "message": "All tools shipped!"})).unwrap();
        assert!(res2.content.contains("completed"));
        assert!(res2.content.contains("Completion summary: All tools shipped!"));

        let res3 = tool.execute(&json!({"blocked_reason": "Missing network access"})).unwrap();
        assert!(res3.content.contains("blocked"));
        assert!(res3.content.contains("Missing network access"));
    }
}
