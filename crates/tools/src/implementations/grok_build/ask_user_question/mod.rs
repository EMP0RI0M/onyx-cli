//! ask_user_question tool — interactive clarification and user prompting.
use crate::{Args, Output, Tool, ToolError};

pub struct AskUserQuestion;

impl Tool for AskUserQuestion {
    fn name(&self) -> &str {
        "ask_user_question"
    }

    fn description(&self) -> &str {
        "Ask the user a structured question or request interactive input when clarification is required. Args: question (string), options (optional array of strings)."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec!["question".into(), "options".into()]
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let question = args.get("question").and_then(|v| v.as_str()).ok_or_else(|| {
            ToolError::BadArgs("missing 'question'".into())
        })?;

        let options_str = if let Some(opts) = args.get("options").and_then(|v| v.as_array()) {
            let list: Vec<String> = opts.iter().filter_map(|o| o.as_str()).map(|s| format!("  - {s}")).collect();
            if list.is_empty() {
                String::new()
            } else {
                format!("\nOptions:\n{}", list.join("\n"))
            }
        } else {
            String::new()
        };

        Ok(Output::ok(format!("Question: {question}{options_str}")))
    }
}
