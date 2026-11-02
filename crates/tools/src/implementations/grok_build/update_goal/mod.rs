//! update_goal tool — model-driven goal tracking and milestone completion.
use crate::{Args, Output, Tool, ToolError};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default)]
pub struct GoalState {
    pub goal: String,
    pub completed: bool,
    pub progress: Vec<String>,
}

#[derive(Clone, Default)]
pub struct UpdateGoalTool {
    state: Arc<Mutex<GoalState>>,
}

impl UpdateGoalTool {
    pub fn new(state: Arc<Mutex<GoalState>>) -> Self {
        Self { state }
    }
}

impl Tool for UpdateGoalTool {
    fn name(&self) -> &str {
        "update_goal"
    }

    fn description(&self) -> &str {
        "Update the active session goal and report progress milestones. Args: goal (string), progress (string), completed (bool)."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec!["goal".into(), "progress".into(), "completed".into()]
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let mut st = self.state.lock().unwrap();
        if let Some(g) = args.get("goal").and_then(|v| v.as_str()) {
            st.goal = g.to_string();
        }
        if let Some(p) = args.get("progress").and_then(|v| v.as_str()) {
            st.progress.push(p.to_string());
        }
        if let Some(c) = args.get("completed").and_then(|v| v.as_bool()) {
            st.completed = c;
        }

        let status = if st.completed { "completed" } else { "in_progress" };
        let msg = format!(
            "Goal: {} [status: {}] ({} progress entries)",
            if st.goal.is_empty() { "(none set)" } else { &st.goal },
            status,
            st.progress.len()
        );
        Ok(Output::ok(msg))
    }
}
