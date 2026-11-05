//! The wire protocol: events, items, turns, approvals.
//!
//! Every type here converts to and from `serde_json::Value` by hand (no derive
//! macros) so the serialized shape is an explicit, reviewable contract — a
//! renamed field becomes a compile error at one `to_json`, not a silent schema
//! drift.

use serde_json::{json, Value};

/// Version of this event schema. Clients may refuse unknown majors.
pub const EVENT_SCHEMA_VERSION: u32 = 1;

/// Identity of one turn inside a thread. Monotonic per thread; `0` is reserved
/// for engine-level notices that belong to no turn (startup info, audits).
pub type TurnId = u64;

/// Lifecycle state of an Item. Not every kind uses every state: a user message
/// appears Completed immediately; streaming assistant text walks
/// Started → Updated* → Completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemStatus {
    Started,
    Updated,
    Completed,
    Failed,
}

impl ItemStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ItemStatus::Started => "started",
            ItemStatus::Updated => "updated",
            ItemStatus::Completed => "completed",
            ItemStatus::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "started" => ItemStatus::Started,
            "updated" => ItemStatus::Updated,
            "completed" => ItemStatus::Completed,
            "failed" => ItemStatus::Failed,
            _ => return None,
        })
    }
}

/// What an Item carries. This is the full render vocabulary any client needs;
/// the TUI maps these onto its own visual cards, an IDE onto panels, a web UI
/// onto collapsible rows.
#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    /// The user's prompt, echoed as an item so replays render identically.
    UserMessage { text: String },
    /// Streaming or final assistant text. `final_text` is set exactly once, on
    /// the Completed/Failed edge, with the whole answer.
    AgentMessage { delta: Option<String>, final_text: Option<String> },
    /// A tool invocation. `output`/`error` appear only on the terminal edges.
    ToolCall { name: String, args: Value, ok: Option<bool>, output: Option<String> },
    /// An engine step beginning — drives spinners/activity labels. `delta`
    /// carries streamed reasoning text on Updated edges so a client can render
    /// live thinking without reopening the answer channel.
    Thinking { step: usize, delta: Option<String> },
    /// Real token usage reported by the model endpoint.
    Usage(UsageFigures),
    /// A notice that is not part of any turn (session path, mode changes).
    Info { text: String },
}

/// Token usage snapshot, kept as plain figures (the engine's own struct stays
/// behind the boundary map).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UsageFigures {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    /// Prompt tokens served from the endpoint's cache this turn. Optional on
    /// the wire: endpoints that don't report it simply omit the field.
    pub cached_prompt_tokens: Option<u64>,
}

/// How a pending approval was resolved by some client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalOutcome {
    Once,
    Always,
    Reject,
}

impl ApprovalOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            ApprovalOutcome::Once => "once",
            ApprovalOutcome::Always => "always",
            ApprovalOutcome::Reject => "reject",
        }
    }
}

/// One envelope on the event stream: an item lifecycle transition inside a
/// turn, plus the thread it belongs to. Clients can key rendering purely off
/// `(thread_id, turn_id, item_id)`.
#[derive(Debug, Clone, PartialEq)]
pub struct CoreEvent {
    pub thread_id: String,
    pub turn_id: TurnId,
    /// Stable id of the item this transition touches, assigned by the host.
    pub item_id: u64,
    pub status: ItemStatus,
    pub kind: ItemKind,
}

impl CoreEvent {
    pub fn new(
        thread_id: impl Into<String>,
        turn_id: TurnId,
        item_id: u64,
        status: ItemStatus,
        kind: ItemKind,
    ) -> Self {
        Self { thread_id: thread_id.into(), turn_id, item_id, status, kind }
    }

    /// Canonical JSON form. `kind` serializes as `{ "type": snake_case, … }`
    /// so clients can dispatch on one string field.
    pub fn to_json(&self) -> Value {
        let kind = match &self.kind {
            ItemKind::UserMessage { text } => json!({ "type": "user_message", "text": text }),
            ItemKind::AgentMessage { delta, final_text } => json!({
                "type": "agent_message",
                "delta": delta,
                "final_text": final_text,
            }),
            ItemKind::ToolCall { name, args, ok, output } => json!({
                "type": "tool_call",
                "name": name,
                "args": args,
                "ok": ok,
                "output": output,
            }),
            ItemKind::Thinking { step, delta } => json!({ "type": "thinking", "step": step, "delta": delta }),
            ItemKind::Usage(u) => json!({
                "type": "usage",
                "prompt_tokens": u.prompt_tokens,
                "completion_tokens": u.completion_tokens,
                "total_tokens": u.total_tokens,
                "cached_prompt_tokens": u.cached_prompt_tokens,
            }),
            ItemKind::Info { text } => json!({ "type": "info", "text": text }),
        };
        json!({
            "v": EVENT_SCHEMA_VERSION,
            "thread_id": self.thread_id,
            "turn_id": self.turn_id,
            "item_id": self.item_id,
            "status": self.status.as_str(),
            "kind": kind,
        })
    }

    /// Inverse of `to_json`; returns `None` on any malformed or unknown-shape
    /// input rather than guessing.
    pub fn from_json(v: &Value) -> Option<Self> {
        if v.get("v").and_then(Value::as_u64)? != EVENT_SCHEMA_VERSION as u64 {
            return None;
        }
        let thread_id = v.get("thread_id")?.as_str()?.to_string();
        let turn_id = v.get("turn_id")?.as_u64()?;
        let item_id = v.get("item_id")?.as_u64()?;
        let status = ItemStatus::parse(v.get("status")?.as_str()?)?;
        let k = v.get("kind")?;
        let ty = k.get("type")?.as_str()?;
        let opt_str = |key: &str| k.get(key).and_then(Value::as_str).map(str::to_string);
        let kind = match ty {
            "user_message" => ItemKind::UserMessage { text: k.get("text")?.as_str()?.to_string() },
            "agent_message" => ItemKind::AgentMessage {
                delta: opt_str("delta"),
                final_text: opt_str("final_text"),
            },
            "tool_call" => ItemKind::ToolCall {
                name: k.get("name")?.as_str()?.to_string(),
                args: k.get("args")?.clone(),
                ok: k.get("ok").and_then(Value::as_bool),
                output: opt_str("output"),
            },
            "thinking" => ItemKind::Thinking {
                step: k.get("step")?.as_u64()? as usize,
                delta: opt_str("delta"),
            },
            "usage" => ItemKind::Usage(UsageFigures {
                prompt_tokens: k.get("prompt_tokens")?.as_u64()?,
                completion_tokens: k.get("completion_tokens")?.as_u64()?,
                total_tokens: k.get("total_tokens")?.as_u64()?,
                cached_prompt_tokens: k.get("cached_prompt_tokens").and_then(Value::as_u64),
            }),
            "info" => ItemKind::Info { text: k.get("text")?.as_str()?.to_string() },
            _ => return None,
        };
        Some(Self { thread_id, turn_id, item_id, status, kind })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_roundtrips_through_json() {
        let ev = CoreEvent::new(
            "t1",
            3,
            7,
            ItemStatus::Completed,
            ItemKind::ToolCall {
                name: "bash".into(),
                args: json!({"command": "ls"}),
                ok: Some(true),
                output: Some("file.txt".into()),
            },
        );
        let back = CoreEvent::from_json(&ev.to_json()).expect("roundtrip");
        assert_eq!(back, ev);
    }

    #[test]
    fn unknown_type_and_bad_version_are_rejected() {
        assert!(CoreEvent::from_json(&json!({"v": 99})).is_none());
        assert!(CoreEvent::from_json(&json!({
            "v": EVENT_SCHEMA_VERSION, "thread_id": "t", "turn_id": 0, "item_id": 0,
            "status": "started", "kind": {"type": "quantum_flux"}
        }))
        .is_none());
    }

    #[test]
    fn streaming_agent_message_shape_is_stable() {
        let ev = CoreEvent::new(
            "t1",
            1,
            2,
            ItemStatus::Updated,
            ItemKind::AgentMessage { delta: Some("hel".into()), final_text: None },
        );
        let j = ev.to_json();
        assert_eq!(j["kind"]["type"], "agent_message");
        assert_eq!(j["kind"]["delta"], "hel");
        assert!(j["kind"]["final_text"].is_null());
        assert_eq!(j["status"], "updated");
    }

    #[test]
    fn null_optional_fields_parse_back_to_none() {
        let ev = CoreEvent::new(
            "t",
            0,
            0,
            ItemStatus::Started,
            ItemKind::AgentMessage { delta: None, final_text: None },
        );
        let back = CoreEvent::from_json(&ev.to_json()).unwrap();
        match back.kind {
            ItemKind::AgentMessage { delta, final_text } => {
                assert!(delta.is_none() && final_text.is_none());
            }
            _ => panic!("wrong kind"),
        }
    }
}
