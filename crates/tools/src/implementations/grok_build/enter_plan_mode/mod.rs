//! Plan Mode tools: EnterPlanMode and ExitPlanMode.
use crate::{Args, Output, Tool, ToolError};
use std::sync::{Arc, Mutex};

#[derive(Default, Clone)]
pub struct PlanState {
    pub active: bool,
    pub plan: String,
}

#[derive(Clone)]
pub struct EnterPlanMode {
    pub state: Arc<Mutex<PlanState>>,
}

impl EnterPlanMode {
    pub fn new(state: Arc<Mutex<PlanState>>) -> Self {
        Self { state }
    }
}

impl Tool for EnterPlanMode {
    fn name(&self) -> &str {
        "enter_plan_mode"
    }

    fn description(&self) -> &str {
        "Enter plan mode to carefully organize complex multi-step tasks before execution. Args: reason (string)."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec!["reason".into()]
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let reason = args.get("reason").and_then(|v| v.as_str()).unwrap_or("planning complex task");
        let mut s = self.state.lock().unwrap();
        s.active = true;
        Ok(Output::ok(format!("entered plan mode: {reason}")))
    }
}

#[derive(Clone)]
pub struct ExitPlanMode {
    pub state: Arc<Mutex<PlanState>>,
}

impl ExitPlanMode {
    pub fn new(state: Arc<Mutex<PlanState>>) -> Self {
        Self { state }
    }
}

impl Tool for ExitPlanMode {
    fn name(&self) -> &str {
        "exit_plan_mode"
    }

    fn description(&self) -> &str {
        "Exit plan mode once planning is complete and begin execution. Args: plan_summary (string)."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec!["plan_summary".into()]
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let summary = args.get("plan_summary").and_then(|v| v.as_str()).unwrap_or("plan completed");
        let mut s = self.state.lock().unwrap();
        s.active = false;
        s.plan = summary.to_string();
        Ok(Output::ok(format!("plan mode exited; proceeding with execution: {summary}")))
    }
}
