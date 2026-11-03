//! tektile-core agent: a minimal OpenManus-style ReAct/tool-call loop in Rust.
//!
//! Design: a `Model` decides the next step; a `ToolCallAgent` runs the loop and
//! routes EVERY tool call through the `Gateway` (policy + audit) — the agent never
//! touches a raw tool. This is the seam where a live LLM (CLIProxyAPI, Gemini, …)
//! plugs in behind the `Model` trait; here we ship a `ScriptedModel` so the whole
//! loop is testable with zero network and zero secrets.

pub mod context;
pub mod core_bridge;
pub mod model;
pub mod session;

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
    DoomLoop { tool: String, repeats: usize, reason: DoomReason },
    Tool(ToolError),
}

/// Why the loop guard fired. Two independent stuck signals (clean-room of the
/// qodercli `doom_loop` decision): the model replaying one exact call batch with
/// no progress, and the model hammering a tool that governance keeps denying —
/// the latter means the permission mode is wrong, not that the model is dumb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoomReason {
    RepeatedCalls,
    RepeatedDenials,
}

impl std::fmt::Display for DoomReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DoomReason::RepeatedCalls => f.write_str("repeating an identical call"),
            DoomReason::RepeatedDenials => {
                f.write_str("hammering a tool that keeps being denied")
            }
        }
    }
}

/// Token usage returned from the model API.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
    pub cached_prompt_tokens: usize,
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
    /// Real token usage reported from the model endpoint.
    Usage(TokenUsage),
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
    /// Consecutive identical tool-call steps tolerated before the loop is
    /// declared stuck. The model repeating one exact call (name + args) with no
    /// progress is a doom loop; 0 disables detection.
    pub doom_limit: usize,
    /// Consecutive *denied* tool calls tolerated before the loop is declared
    /// stuck. A denial is fed back to the model as a tool result rather than
    /// aborting the turn, so without this a model that ignores the refusal would
    /// retry forever against an unchanging policy. 0 disables detection.
    pub denial_limit: usize,
    pub system: String,
}

impl<'a, P: Policy + ?Sized> ToolCallAgent<'a, P> {
    pub fn new(gateway: &'a Gateway<P>) -> Self {
        Self {
            gateway,
            max_steps: 8,
            doom_limit: 3,
            denial_limit: 4,
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
        // Skip the push when the caller already appended this exact last user
        // message (the TUI persists the prompt before calling in).
        if history.last().map_or(true, |m| m.role != "user" || m.content != user) {
            history.push(Message::text("user", user));
        }
        let tool_names = self.gateway.tools.names();
        let tools: Vec<&str> = tool_names.iter().map(|s| s.as_str()).collect();

        // Doom-loop guard: if the model emits the exact same batch of tool calls
        // (names + args, in order) on `doom_limit` consecutive steps, it is stuck
        // repeating itself with no progress — stop rather than burn the budget.
        let mut last_sig: Option<String> = None;
        let mut repeat_run = 0usize;
        // Denial-streak guard (see `denial_limit`): consecutive steps in which at
        // least one call was refused by policy.
        let mut denial_run = 0usize;

        for step in 0..self.max_steps {
            on_event(AgentEvent::Thinking { step });
            let action = if stream {
                let mut sink = |d: &str| on_event(AgentEvent::TextDelta(d.to_string()));
                let hm = model
                    .as_any_mut()
                    .downcast_mut::<crate::model::HttpModel>()
                    .unwrap();
                let res = hm.next_step_streaming(history, &tools, &mut sink);
                if let Some(u) = &hm.last_usage {
                    on_event(AgentEvent::Usage(u.clone()));
                }
                res
            } else {
                model.next_step(history, &tools)
            };
            match action {
                Step::Final(answer) => {
                    history.push(Message::text("assistant", answer.clone()));
                    return Ok(RunReport { answer, steps: step + 1, history: history.clone() });
                }
                Step::Calls(calls) => {
                    let sig = calls
                        .iter()
                        .map(|c| format!("{}({})", c.name, c.args))
                        .collect::<Vec<_>>()
                        .join("|");
                    if self.doom_limit > 0 && Some(&sig) == last_sig.as_ref() {
                        repeat_run += 1;
                        if repeat_run >= self.doom_limit {
                            return Err(AgentError::DoomLoop {
                                tool: calls[0].name.clone(),
                                repeats: repeat_run,
                                reason: DoomReason::RepeatedCalls,
                            });
                        }
                    } else {
                        repeat_run = 0;
                        last_sig = Some(sig);
                    }
                    let mut terminate_answer: Option<String> = None;
                    // A denial is not a hard stop — it is fed back to the model as
                    // a tool result so it can pick another path. We only count the
                    // *consecutive* run across the whole batch (denials interleaved
                    // with successes mean the agent is making progress).
                    let mut denied_this_step = false;
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
                        let content = match outcome {
                            Ok(out) => {
                                if call.name == "terminate" {
                                    terminate_answer = Some(out.content.clone());
                                }
                                crate::context::shrink_tool_output(
                                    &out.content,
                                    24_000,
                                    12_000,
                                )
                            }
                            Err(ToolError::Denied(rule)) => {
                                denied_this_step = true;
                                format!(
                                    "DENIED by policy: {rule}\n\
                                     The user's governance rules will keep refusing this \
                                     exact call. Do NOT retry it unchanged — choose a \
                                     different tool or argument, explain why you need it, or \
                                     finish without it."
                                )
                            }
                            Err(e) => return Err(AgentError::Tool(e)),
                        };
                        history.push(Message::text("tool", content));
                        if terminate_answer.is_some() {
                            break; // terminate ends the turn
                        }
                    }
                    if self.denial_limit > 0 {
                        if denied_this_step {
                            denial_run += 1;
                            if denial_run >= self.denial_limit {
                                return Err(AgentError::DoomLoop {
                                    tool: String::new(),
                                    repeats: denial_run,
                                    reason: DoomReason::RepeatedDenials,
                                });
                            }
                        } else {
                            denial_run = 0;
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
        // BashGuard denies; the denial is fed back to the model as a tool result
        // (not a thrown error), so the script exhausts into a Final answer. The
        // invariant is that the dangerous command NEVER ran — proven by the audit
        // recording a DENY, not an OK/FAIL for bash.
        let rep = agent.run(&mut model, "destroy everything").unwrap();
        assert_eq!(rep.answer, "(script exhausted)");
        let audit = gw.audit.snapshot();
        assert!(audit.iter().any(|e| e.tool == "bash" && e.outcome.starts_with("DENY")), "{audit:?}");
        // the command never executed: no OK/FAIL/AUTO outcome for bash
        assert!(!audit.iter().any(|e| e.tool == "bash" && !e.outcome.starts_with("DENY")), "{audit:?}");
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

    #[test]
    fn doom_loop_detected_on_repeated_identical_calls() {
        let gw = gateway();
        // The model repeats one exact vecmath call forever; the loop must stop it.
        let same = || Step::Calls(vec![ToolCall {
            name: "vecmath".into(),
            args: json!({"op":"dot","a":[1.0],"b":[1.0]}),
        }]);
        let mut model = ScriptedModel::new(vec![same(), same(), same(), same(), same()]);
        let mut agent = ToolCallAgent::new(&gw);
        agent.doom_limit = 3;
        agent.max_steps = 8;
        let res = agent.run(&mut model, "loop forever");
        assert!(
            matches!(res, Err(AgentError::DoomLoop { tool: ref t, repeats, reason: DoomReason::RepeatedCalls }) if t == "vecmath" && repeats == 3),
            "{res:?}"
        );
    }

    #[test]
    fn repeated_denials_are_fed_back_then_intercepted() {
        let gw = gateway();
        // BashGuard refuses `rm -rf /`; the model keeps trying it unchanged. The
        // loop must NOT abort on the first denial — it feeds the refusal back as a
        // tool result so the model can adapt, and only fires once the streak hits
        // the limit.
        let rm = || Step::Calls(vec![ToolCall {
            name: "bash".into(),
            args: json!({"command": "rm -rf /"}),
        }]);
        let mut model = ScriptedModel::new(vec![rm(), rm(), rm(), rm(), rm()]);
        let mut agent = ToolCallAgent::new(&gw);
        agent.doom_limit = 0; // isolate the denial signal from the repeat signal
        agent.denial_limit = 4;
        agent.max_steps = 8;
        let res = agent.run(&mut model, "destroy everything");
        assert!(
            matches!(res, Err(AgentError::DoomLoop { repeats, reason: DoomReason::RepeatedDenials, .. }) if repeats == 4),
            "{res:?}"
        );
    }

    #[test]
    fn single_denial_does_not_abort_the_turn() {
        let gw = gateway();
        // One denial, then the model adapts and finishes. A lone refusal is a
        // recoverable signal, not a hard stop.
        let mut model = ScriptedModel::new(vec![
            Step::Calls(vec![ToolCall { name: "bash".into(), args: json!({"command": "rm -rf /"}) }]),
            Step::Final("I cannot delete the filesystem; here is a safe alternative.".into()),
        ]);
        let agent = ToolCallAgent::new(&gw);
        let rep = agent.run(&mut model, "destroy everything").unwrap();
        assert!(rep.answer.contains("safe alternative"));
        // the denial was recorded in history as a tool message, not thrown
        assert!(rep.history.iter().any(|m| m.role == "tool" && m.content.starts_with("DENIED by policy")));
    }

    #[test]
    fn varying_calls_are_not_a_doom_loop() {
        let gw = gateway();
        // Same tool, different args each step — real progress, must not trip.
        let mut model = ScriptedModel::new(vec![
            Step::Calls(vec![ToolCall { name: "vecmath".into(), args: json!({"op":"dot","a":[1.0],"b":[1.0]}) }]),
            Step::Calls(vec![ToolCall { name: "vecmath".into(), args: json!({"op":"dot","a":[2.0],"b":[2.0]}) }]),
            Step::Calls(vec![ToolCall { name: "vecmath".into(), args: json!({"op":"dot","a":[3.0],"b":[3.0]}) }]),
            Step::Final("done".into()),
        ]);
        let mut agent = ToolCallAgent::new(&gw);
        agent.doom_limit = 3;
        agent.max_steps = 8;
        let rep = agent.run(&mut model, "varying dots").unwrap();
        assert_eq!(rep.answer, "done");
    }
}
