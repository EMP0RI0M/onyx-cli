//! ask_user_question tool — interactive clarification and user prompting.
use crate::{Args, Output, Tool, ToolError};
use std::sync::Arc;

pub type QuestionHook = Arc<dyn Fn(&str, &[String]) -> Result<String, String> + Send + Sync>;

static QUESTION_HOOK: std::sync::RwLock<Option<QuestionHook>> = std::sync::RwLock::new(None);

pub fn set_question_hook(hook: QuestionHook) {
    if let Ok(mut w) = QUESTION_HOOK.write() {
        *w = Some(hook);
    }
}

pub fn clear_question_hook() {
    if let Ok(mut w) = QUESTION_HOOK.write() {
        *w = None;
    }
}

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

        let options: Vec<String> = if let Some(opts) = args.get("options").and_then(|v| v.as_array()) {
            opts.iter().filter_map(|o| o.as_str().map(|s| s.to_string())).collect()
        } else {
            Vec::new()
        };

        // If an interactive hook is registered (e.g. from TUI), delegate to it and suspend
        if let Ok(r) = QUESTION_HOOK.read() {
            if let Some(hook) = r.as_ref() {
                match hook(question, &options) {
                    Ok(answer) => return Ok(Output::ok(answer)),
                    Err(e) => return Ok(Output::err(e)),
                }
            }
        }

        let options_str = if options.is_empty() {
            String::new()
        } else {
            let list: Vec<String> = options.iter().map(|s| format!("  - {s}")).collect();
            format!("\nOptions:\n{}", list.join("\n"))
        };

        Ok(Output::ok(format!("Question: {question}{options_str}")))
    }
}
