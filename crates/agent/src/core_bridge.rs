//! Boundary map between the engine's internal types and the onyx-core wire
//! protocol. This is the ONLY place that knows both sides — clients consume
//! `CoreEvent`/`ChatMessage`, the loop keeps emitting `AgentEvent`/`Message`.

use crate::{AgentEvent, Message};
use onyx_core::protocol::{CoreEvent, ItemKind, ItemStatus, UsageFigures};
use onyx_core::session::ChatMessage;

impl AgentEvent {
    /// Project one engine event onto the protocol envelope for `thread_id` /
    /// `turn_id`. `item_id` is assigned by the host (it tracks which item a
    /// stream belongs to); this function only maps kind + status.
    pub fn to_core(&self, thread_id: &str, turn_id: u64, item_id: u64) -> CoreEvent {
        let (status, kind) = match self {
            AgentEvent::Thinking { step } => (
                ItemStatus::Started,
                ItemKind::Thinking { step: *step, delta: None },
            ),
            AgentEvent::ReasoningDelta(text) => (
                ItemStatus::Updated,
                ItemKind::Thinking { step: 0, delta: Some(text.clone()) },
            ),
            AgentEvent::TextDelta(text) => (
                ItemStatus::Updated,
                ItemKind::AgentMessage { delta: Some(text.clone()), final_text: None },
            ),
            AgentEvent::ToolStart { name, args } => (
                ItemStatus::Started,
                ItemKind::ToolCall {
                    name: name.clone(),
                    args: args.clone(),
                    ok: None,
                    output: None,
                },
            ),
            AgentEvent::ToolDone { name, ok, output } => (
                if *ok { ItemStatus::Completed } else { ItemStatus::Failed },
                ItemKind::ToolCall {
                    name: name.clone(),
                    args: serde_json::Value::Null,
                    ok: Some(*ok),
                    output: Some(output.clone()),
                },
            ),
            AgentEvent::Usage(u) => (
                ItemStatus::Completed,
                ItemKind::Usage(UsageFigures {
                    prompt_tokens: u.prompt_tokens as u64,
                    completion_tokens: u.completion_tokens as u64,
                    total_tokens: u.total_tokens as u64,
                    cached_prompt_tokens: Some(u.cached_prompt_tokens as u64),
                }),
            ),
            AgentEvent::CapabilitySelection(text) => (
                ItemStatus::Completed,
                ItemKind::Info { text: text.clone() },
            ),
            AgentEvent::StallDetected { phrase, step } => (
                ItemStatus::Failed,
                ItemKind::Info { text: format!("Generation stall detected at step {step}: repeating '{phrase}'") },
            ),
            AgentEvent::StepCompleted { step, summary } => (
                ItemStatus::Completed,
                ItemKind::Info { text: format!("Step {step}: {summary}") },
            ),
        };
        CoreEvent::new(thread_id, turn_id, item_id, status, kind)
    }
}

impl From<&Message> for ChatMessage {
    fn from(m: &Message) -> Self {
        ChatMessage {
            role: m.role.clone(),
            content: m.content.clone(),
            tool_call: m.tool_call.as_ref().map(|tc| (tc.name.clone(), tc.args.clone())),
        }
    }
}

impl From<&ChatMessage> for Message {
    fn from(c: &ChatMessage) -> Self {
        let mut m = Message::text(&c.role, c.content.clone());
        if let Some((name, args)) = &c.tool_call {
            m.tool_call = Some(crate::ToolCall { name: name.clone(), args: args.clone() });
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_agent_event_variant_projects() {
        let cases = vec![
            AgentEvent::Thinking { step: 2 },
            AgentEvent::ReasoningDelta("hmm".into()),
            AgentEvent::TextDelta("partial".into()),
            AgentEvent::ToolStart { name: "bash".into(), args: json!({"command": "ls"}) },
            AgentEvent::ToolDone { name: "bash".into(), ok: false, output: "denied".into() },
            AgentEvent::Usage(crate::TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 4,
                total_tokens: 14,
                cached_prompt_tokens: 2,
            }),
        ];
        for ev in cases {
            let core = ev.to_core("t1", 1, 3);
            assert_eq!(core.thread_id, "t1");
            assert_eq!(core.item_id, 3);
            // The projection must survive JSON round-trip unchanged.
            let back = CoreEvent::from_json(&core.to_json()).expect("wire roundtrip");
            assert_eq!(back.kind, core.kind);
        }
    }

    #[test]
    fn tool_done_failure_maps_to_failed_status() {
        let ev = AgentEvent::ToolDone { name: "x".into(), ok: false, output: "no".into() };
        let core = ev.to_core("t", 0, 0);
        assert_eq!(core.status, ItemStatus::Failed);
    }

    #[test]
    fn reasoning_delta_projects_as_updated_thinking_not_answer() {
        let core = AgentEvent::ReasoningDelta("let me think".into()).to_core("t", 1, 4);
        assert_eq!(core.status, ItemStatus::Updated);
        // Must be a Thinking item carrying the delta, NOT an AgentMessage.
        match &core.kind {
            ItemKind::Thinking { delta: Some(d), .. } => assert_eq!(d, "let me think"),
            other => panic!("expected Thinking with delta, got {other:?}"),
        }
        let back = CoreEvent::from_json(&core.to_json()).expect("wire roundtrip");
        assert_eq!(back.kind, core.kind);
    }

    #[test]
    fn message_conversion_is_lossless_both_ways() {
        let mut m = Message::text("assistant", "calling tool");
        m.tool_call = Some(crate::ToolCall { name: "read_file".into(), args: json!({"p": 1}) });
        let chat = ChatMessage::from(&m);
        let back = Message::from(&chat);
        assert_eq!(back.role, m.role);
        assert_eq!(back.content, m.content);
        assert_eq!(back.tool_call.unwrap().name, "read_file");
    }
}
