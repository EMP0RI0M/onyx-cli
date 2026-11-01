//! Control tools — OpenManus `Terminate` and `AskHuman`.
//!
//! `terminate` signals the agent loop to stop and hand back a final answer.
//! `ask_human` returns a marker the loop can surface to a human and block on.

use crate::{Args, Output, Tool, ToolError};

pub struct Terminate;

impl Tool for Terminate {
    fn name(&self) -> &str {
        "terminate"
    }
    fn arg_keys(&self) -> Vec<String> {
        vec!["status".into(), "content".into()]
    }
    fn description(&self) -> &str {
        "Signal completion. Provide status ('success'|'failure') and the final answer."
    }
    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let status = args.get("status").and_then(|v| v.as_str()).unwrap_or("success");
        let answer = args
            .get("content")
            .and_then(|v| v.as_str())
            .or_else(|| args.get("answer").and_then(|v| v.as_str()))
            .unwrap_or("");
        Ok(match status {
            "failure" => Output::err(answer),
            _ => Output::ok(answer),
        })
    }
}

pub struct AskHuman;

impl Tool for AskHuman {
    fn name(&self) -> &str {
        "ask_human"
    }
    fn arg_keys(&self) -> Vec<String> {
        vec!["question".into()]
    }
    fn description(&self) -> &str {
        "Ask the user a question and pause for their reply."
    }
    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let question = args
            .get("question")
            .and_then(|v| v.as_str())
            .or_else(|| args.get("content").and_then(|v| v.as_str()))
            .ok_or_else(|| ToolError::BadArgs("missing `question`".into()))?;
        Ok(Output::ok(format!("[ask_human] {question}")))
    }
}
