//! `send_feedback` tool — saves or updates user/system feedback drafts for inspection.
use crate::{Args, Output, Tool, ToolError};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub const SEND_FEEDBACK_TOOL_NAME: &str = "send_feedback";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackDraft {
    pub id: String,
    pub title: String,
    pub details: String,
    pub product_area: Option<String>,
    pub feedback_type: String,
    pub task_category: Option<String>,
    pub created_at_secs: u64,
}

#[derive(Default)]
pub struct SendFeedbackTool;

impl Tool for SendFeedbackTool {
    fn name(&self) -> &str {
        SEND_FEEDBACK_TOOL_NAME
    }

    fn description(&self) -> &str {
        "Save or update a structured feedback draft for bugs, unexpected model behavior, or capability requests. Args: title (string), details (string), type (string, e.g. 'bug'|'feature'), product_area (optional string)."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec![
            "title".into(),
            "details".into(),
            "type".into(),
            "product_area".into(),
        ]
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let title = args.get("title").and_then(|v| v.as_str()).ok_or_else(|| {
            ToolError::BadArgs("missing 'title'".into())
        })?;
        let details = args.get("details").and_then(|v| v.as_str()).ok_or_else(|| {
            ToolError::BadArgs("missing 'details'".into())
        })?;
        let fb_type = args.get("type").and_then(|v| v.as_str()).unwrap_or("bug");
        let product_area = args.get("product_area").and_then(|v| v.as_str()).map(str::to_string);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let draft_id = format!("fb_{:x}", now);
        let draft = FeedbackDraft {
            id: draft_id.clone(),
            title: title.to_string(),
            details: details.to_string(),
            product_area,
            feedback_type: fb_type.to_string(),
            task_category: None,
            created_at_secs: now,
        };

        // Persist to ~/.onyx/feedback.jsonl
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        let dir = home.join(".onyx");
        let _ = std::fs::create_dir_all(&dir);
        let file_path = dir.join("feedback.jsonl");

        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&file_path) {
            if let Ok(line) = serde_json::to_string(&draft) {
                let _ = writeln!(f, "{line}");
            }
        }

        Ok(Output::ok(format!(
            "Feedback draft saved with ID: {draft_id}\nTitle: \"{title}\"\nType: {fb_type}\nLocation: {}",
            file_path.display()
        )))
    }
}
