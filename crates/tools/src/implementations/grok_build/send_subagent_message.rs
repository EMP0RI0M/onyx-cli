//! `send_subagent_message` tool — inter-agent communication, delegation, and steering.
use crate::{Args, Output, Tool, ToolError};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

pub const SEND_SUBAGENT_MESSAGE_TOOL_NAME: &str = "send_subagent_message";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentMessage {
    pub subagent_id: String,
    pub text: String,
    pub delivery: String,
    pub timestamp_secs: u64,
}

#[derive(Default)]
pub struct SubagentMessageHub {
    pub outbox: Mutex<Vec<SubagentMessage>>,
}

impl SubagentMessageHub {
    pub fn new() -> Self {
        Self {
            outbox: Mutex::new(Vec::new()),
        }
    }
}

pub static GLOBAL_SUBAGENT_HUB: std::sync::LazyLock<SubagentMessageHub> =
    std::sync::LazyLock::new(SubagentMessageHub::new);

#[derive(Default)]
pub struct SendSubagentMessageTool;

impl Tool for SendSubagentMessageTool {
    fn name(&self) -> &str {
        SEND_SUBAGENT_MESSAGE_TOOL_NAME
    }

    fn description(&self) -> &str {
        "Send an instruction or feedback message to an active or background subagent. Args: subagent_id (string), text (string), delivery (optional 'steer'|'queue'|'interject')."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec!["subagent_id".into(), "text".into(), "delivery".into()]
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let subagent_id = args.get("subagent_id").and_then(|v| v.as_str()).ok_or_else(|| {
            ToolError::BadArgs("missing 'subagent_id'".into())
        })?;
        let text = args.get("text").and_then(|v| v.as_str()).ok_or_else(|| {
            ToolError::BadArgs("missing 'text'".into())
        })?;
        let delivery = args.get("delivery").and_then(|v| v.as_str()).unwrap_or("steer");

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let msg = SubagentMessage {
            subagent_id: subagent_id.to_string(),
            text: text.to_string(),
            delivery: delivery.to_string(),
            timestamp_secs: now,
        };

        if let Ok(mut outbox) = GLOBAL_SUBAGENT_HUB.outbox.lock() {
            outbox.push(msg);
        }

        Ok(Output::ok(format!(
            "Message dispatched to subagent '{subagent_id}' via mode '{delivery}'.\nPayload length: {} bytes.",
            text.len()
        )))
    }
}
