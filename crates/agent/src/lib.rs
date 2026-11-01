//! tektile-core agent: a minimal OpenManus-style ReAct/tool-call loop in Rust.
//!
//! Design: a `Model` decides the next step; a `ToolCallAgent` runs the loop and
//! routes EVERY tool call through the `Gateway` (policy + audit) — the agent never
//! touches a raw tool. This is the seam where a live LLM (CLIProxyAPI, Gemini, …)
//! plugs in behind the `Model` trait; here we ship a `ScriptedModel` so the whole
//! loop is testable with zero network and zero secrets.

pub mod model;

use serde_json::Value;
use std::collections::VecDeque;
use tools::policy::Policy;
use tools::audit::Gateway;
use tools::ToolError;

/// A single conversational turn.
#[derive(Debug, Clone)]
pub struct Message {
    pub role: String,   // "system" | "user" | "assistant" | "tool"
    pub content: String,
    pub tool_call: Option<ToolCall>,
}

impl Message {
    pub fn text(role: &str, content: impl Into<String>) -> Self {
        Self { role: role.into(), content: content.into(), tool_call: None }
    }
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: String,
    pub args: Value,
}

/// What a model emits on a step: one or more tool calls, or a final text answer.
#[derive(Debug, Clone)]
pub enum Step {
    Calls(Vec<ToolCall>),
    Final(String),
}

impl Step {
    /// Convenience for single-call construction (tests, scripted models).
    pub fn call(name: &str, args: Value) -> Self {
        Step::Calls(vec![ToolCall { name: name.to_string(), args }])
    }
}

/// The decision engine behind the agent loop. Implementations turn a history
/// plus the live tool inventory into the next `Step`. The `as_any*` hooks let
/// observers (the TUI) reach a concrete model — e.g. `HttpModel`'s streaming
/// path — without any downcast machinery at every call site.
pub trait Model {
    fn next_step(&mut self, history: &[Message], tools: &[&str]) -> Step;
    fn as_any(&self) -> &dyn std::any::Any;
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

#[derive(Debug)]
pub enum AgentError {
    MaxSteps,
    Tool(ToolError),
}

/// Observational events emitted while a turn runs. A UI (the onyx TUI) renders
/// these; `run_turn` stays silent so tests, agentd and subagents are unaffected.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// A model step is starting (spinner + activity label).
    Thinking { step: usize },
    /// Streamed assistant/reasoning text delta.
    TextDelta(String),
    /// The model requested this tool call.
    ToolStart { name: String, args: Value },
    /// The gateway returned for this call (Err arm = denied or failed).
    ToolDone { name: String, ok: bool, output: String },
}

/// Result of a full run: the final answer and the total steps taken.
#[derive(Debug)]
pub struct RunReport {
    pub answer: String,
    pub steps: usize,
    pub history: Vec<Message>,
}

pub struct ToolCallAgent<'a, P: Policy + ?Sized> {
    pub gateway: &'a Gateway<P>,
    pub max_steps: usize,
    pub system: String,
}

impl<'a, P: Policy + ?Sized> ToolCallAgent<'a, P> {
    pub fn new(gateway: &'a Gateway<P>) -> Self {
        Self {
            gateway,
            max_steps: 8,
            system: "You are tektile-core, a precise agent. Use tools to act; when done, \
                     call terminate with the final answer."
                .into(),
        }
    }

    /// Drive the loop for one user turn. Every tool call is policy-checked and
    /// audited by the Gateway. A `terminate` tool call ends the run immediately.
    pub fn run(&self, model: &mut dyn Model, user: &str) -> Result<RunReport, AgentError> {
        let mut history = vec![Message::text("system", self.system.clone())];
        self.run_turn(model, user, &mut history)
    }

    /// Like `run`, but appends to a persistent conversation history so a REPL
    /// keeps context across turns. The system message must already be present.
    pub fn run_turn(
        &self,
        model: &mut dyn Model,
        user: &str,
        history: &mut Vec<Message>,
    ) -> Result<RunReport, AgentError> {
        self.run_turn_observed(model, user, history, &mut |_| {})
    }

    /// `run_turn` with an observer: same loop and history bookkeeping, plus an
    /// `AgentEvent` per step/tool call. When the model is an `HttpModel`, its
    /// streaming path feeds `TextDelta` events as tokens arrive.
    pub fn run_turn_observed(
        &self,
        model: &mut dyn Model,
        user: &str,
        history: &mut Vec<Message>,
        on_event: &mut dyn FnMut(AgentEvent),
    ) -> Result<RunReport, AgentError> {
        let stream = model
            .as_any_mut()
            .downcast_mut::<crate::model::HttpModel>()
            .is_some();
        history.push(Message::text("user", user));
        let tool_names = self.gateway.tools.names();
        let tools: Vec<&str> = tool_names.iter().map(|s| s.as_str()).collect();

        for step in 0..self.max_steps {
            on_event(AgentEvent::Thinking { step });
            let action = if stream {
                let mut sink = |d: &str| on_event(AgentEvent::TextDelta(d.to_string()));
                model
                    .as_any_mut()
                    .downcast_mut::<crate::model::HttpModel>()
                    .unwrap()
                    .next_step_streaming(history, &tools, &mut sink)
            } else {
                model.next_step(history, &tools)
            };
            match action {
                Step::Final(answer) => {
                    history.push(Message::text("assistant", answer.clone()));
                    return Ok(RunReport { answer, steps: step + 1, history: history.clone() });
                }
                Step::Calls(calls) => {
                    let mut terminate_answer: Option<String> = None;
                    for call in calls {
                        on_event(AgentEvent::ToolStart { name: call.name.clone(), args: call.args.clone() });
                        history.push(Message {
                            role: "assistant".into(),
                            content: format!("→ {}({})", call.name, call.args),
                            tool_call: Some(call.clone()),
                        });
                        let outcome = self.gateway.call(&call.name, &call.args);
                        match &outcome {
                            Ok(out) => on_event(AgentEvent::ToolDone {
                                name: call.name.clone(),
                                ok: !out.is_error,
                                output: out.content.clone(),
                            }),
                            Err(e) => on_event(AgentEvent::ToolDone {
                                name: call.name.clone(),
                                ok: false,
                                output: e.to_string(),
                            }),
                        }
                        let output = outcome.map_err(AgentError::Tool)?;
                        history.push(Message::text("tool", output.content.clone()));
                        if call.name == "terminate" {
                            terminate_answer = Some(output.content);
                            break; // terminate ends the turn
                        }
                    }
                    if let Some(answer) = terminate_answer {
                        return Ok(RunReport { answer, steps: step + 1, history: history.clone() });
                    }
                }
            }
        }
        Err(AgentError::MaxSteps)
    }
}

/// A `Model` that replays a fixed script of steps — lets the whole loop run
/// deterministically in tests (and as an offline demo) without any LLM.
pub struct ScriptedModel {
    pub steps: VecDeque<Step>,
}

impl ScriptedModel {
    pub fn new(steps: Vec<Step>) -> Self {
        Self { steps: steps.into() }
    }
}

impl Model for ScriptedModel {
    fn next_step(&mut self, _history: &[Message], _tools: &[&str]) -> Step {
        self.steps
            .pop_front()
            .unwrap_or_else(|| Step::Final("(script exhausted)".into()))
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// Run one scripted subagent entry through a gateway: builds its own
/// `ToolCallAgent`, drives `steps` as the model, and returns the report plus the
/// gateway's audit trail. Lives here (not in `subagent`) because only this crate
/// can name the generic `Gateway<P>`; used by tests and offline demos.
pub fn run_scripted_entry<P: Policy + ?Sized>(
    gateway: &Gateway<P>,
    steps: Vec<Step>,
    task: &str,
    max_steps: usize,
) -> Result<(RunReport, Vec<tools::audit::AuditEntry>), AgentError> {
    let mut model = ScriptedModel::new(steps);
    let mut agent = ToolCallAgent::new(gateway);
    agent.max_steps = max_steps;
    let rep = agent.run(&mut model, task)?;
    Ok((rep, gateway.audit.snapshot()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use tools::policy::BashGuard;

    fn gateway() -> Gateway<BashGuard> {
        Gateway::new(tools::default_collection(PathBuf::from("/tmp")), BashGuard::default())
    }

    #[test]
    fn loop_routes_tool_call_through_gateway_then_terminates() {
        let gw = gateway();
        let mut model = ScriptedModel::new(vec![
            Step::Calls(vec![ToolCall { name: "vecmath".into(), args: json!({"op":"dot","a":[1.0,2.0,3.0],"b":[4.0,5.0,6.0]}) }]),
            Step::Calls(vec![ToolCall { name: "terminate".into(), args: json!({"status":"success","content":"dot is 32"}) }]),
        ]);
        let agent = ToolCallAgent::new(&gw);
        let rep = agent.run(&mut model, "dot product of [1,2,3]·[4,5,6]").unwrap();

        assert_eq!(rep.answer, "dot is 32");
        // vecmath ran and produced "32" as a tool message in history.
        let tool_msgs: Vec<&Message> = rep.history.iter().filter(|m| m.role == "tool").collect();
        assert!(tool_msgs.iter().any(|m| m.content.trim() == "32"));
        // Both calls were audited by the gateway.
        assert_eq!(gw.audit.snapshot().len(), 2);
    }

    #[test]
    fn multiple_calls_in_one_step_all_execute() {
        let gw = gateway();
        let mut model = ScriptedModel::new(vec![Step::Calls(vec![
            ToolCall { name: "vecmath".into(), args: json!({"op":"dot","a":[1.0],"b":[2.0]}) },
            ToolCall { name: "terminate".into(), args: json!({"status":"success","content":"done"}) },
        ])]);
        let agent = ToolCallAgent::new(&gw);
        let rep = agent.run(&mut model, "dot then stop").unwrap();
        assert_eq!(rep.answer, "done");
        assert_eq!(gw.audit.snapshot().len(), 2);
    }

    #[test]
    fn run_turn_keeps_history_across_turns() {
        let gw = gateway();
        let agent = ToolCallAgent::new(&gw);
        let mut history = vec![Message::text("system", agent.system.clone())];
        let mut m1 = ScriptedModel::new(vec![Step::Final("first".into())]);
        agent.run_turn(&mut m1, "q1", &mut history).unwrap();
        let mut m2 = ScriptedModel::new(vec![Step::Final("second".into())]);
        let rep = agent.run_turn(&mut m2, "q2", &mut history).unwrap();
        assert_eq!(rep.answer, "second");
        // both user turns present in the shared history
        assert_eq!(rep.history.iter().filter(|m| m.role == "user").count(), 2);
    }

    #[test]
    fn loop_denied_tool_never_executes() {
        let gw = gateway();
        let mut model = ScriptedModel::new(vec![
            Step::Calls(vec![ToolCall { name: "bash".into(), args: json!({"command":"rm -rf /"}) }]),
        ]);
        let agent = ToolCallAgent::new(&gw);
        // BashGuard denies; the gateway surfaces a Denied error to the agent.
        let res = agent.run(&mut model, "destroy everything");
        assert!(matches!(res, Err(AgentError::Tool(ToolError::Denied(_)))));
    }

    #[test]
    fn plain_final_answer_needs_no_tools() {
        let gw = gateway();
        let mut model = ScriptedModel::new(vec![Step::Final("2+2=4".into())]);
        let agent = ToolCallAgent::new(&gw);
        let rep = agent.run(&mut model, "what is 2+2").unwrap();
        assert_eq!(rep.answer, "2+2=4");
        assert_eq!(rep.steps, 1);
    }
}
