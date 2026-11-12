//! tektile-core agent: a minimal OpenManus-style ReAct/tool-call loop in Rust.
//!
//! Design: a `Model` decides the next step; a `ToolCallAgent` runs the loop and
//! routes EVERY tool call through the `Gateway` (policy + audit) — the agent never
//! touches a raw tool. This is the seam where a live LLM (CLIProxyAPI, Gemini, …)
//! plugs in behind the `Model` trait; here we ship a `ScriptedModel` so the whole
//! loop is testable with zero network and zero secrets.

pub mod autonomous;
pub mod context;
pub mod context_assembler;
pub mod core_bridge;
pub mod discovery;
pub mod model;
pub mod provider;
pub mod retrieval;
pub mod repetition;
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
    /// Streamed reasoning/thinking text delta, kept separate from the answer so a
    /// UI can render it as collapsible "thinking" rather than the final message.
    ReasoningDelta(String),
    /// Streamed assistant/reasoning text delta.
    TextDelta(String),
    /// The model requested this tool call.
    ToolStart { name: String, args: Value },
    /// The gateway returned for this call (Err arm = denied or failed).
    ToolDone { name: String, ok: bool, output: String },
    /// Real token usage reported from the model endpoint.
    Usage(TokenUsage),
    /// Retrieval narrowed this turn's tool surface; a one-line status the UI can
    /// show so the operator sees *why* only some tools were offered.
    CapabilitySelection(String),
    /// Repetition stall detected during generation (repeated n-gram loop).
    StallDetected { phrase: String, step: usize },
    /// Concise per-step decision summary and operational trace.
    StepCompleted { step: usize, summary: String },
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
    /// Retrieval-backed capability selection. When set, each turn projects the
    /// tool surface down to what the user's intent matches before the model sees
    /// it (see [`crate::retrieval`]). `None` keeps the full catalog on every step —
    /// the default, and what small catalogs and non-`HttpModel` callers want.
    pub selector: Option<&'a crate::retrieval::CapabilitySelector>,
    /// Task-aware context assembler for project intelligence and architectural awareness.
    pub context_assembler: Option<&'a crate::context_assembler::ContextAssembler>,
    /// Active task checkpoint for autonomous execution, failure diagnosis, and recovery.
    pub checkpoint: Option<std::sync::Arc<std::sync::Mutex<crate::autonomous::TaskCheckpoint>>>,
}

impl<'a, P: Policy + ?Sized> ToolCallAgent<'a, P> {
    pub fn new(gateway: &'a Gateway<P>) -> Self {
        Self {
            gateway,
            max_steps: 8,
            doom_limit: 3,
            denial_limit: 4,
            selector: None,
            context_assembler: None,
            checkpoint: None,
            system: "You are tektile-core, a precise agent. Use tools to act; when done, \
                     call terminate with the final answer."
                .into(),
        }
    }

    pub fn with_checkpoint(mut self, cp: std::sync::Arc<std::sync::Mutex<crate::autonomous::TaskCheckpoint>>) -> Self {
        self.checkpoint = Some(cp);
        self
    }

    pub fn with_context_assembler(mut self, assembler: &'a crate::context_assembler::ContextAssembler) -> Self {
        self.context_assembler = Some(assembler);
        self
    }

    /// Run the loop with retrieval-driven per-turn tool selection.
    pub fn with_selector(mut self, selector: &'a crate::retrieval::CapabilitySelector) -> Self {
        self.selector = Some(selector);
        self
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
        mut on_event: &mut dyn FnMut(AgentEvent),
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

        // Retrieval: project this turn's tool surface down to what the intent
        // matches. Only meaningful for an HttpModel (the only thing that renders a
        // catalog); scripted/test models keep the full gateway inventory below.
        // apply_turn writes hm.turn_tools; we read the offered names back out so the
        // loop's own tool list agrees with exactly what the model was shown.
        let mut offered_override: Option<Vec<String>> = None;
        if let Some(sel) = self.selector {
            if let Some(hm) = model.as_any_mut().downcast_mut::<crate::model::HttpModel>() {
                sel.apply_turn(hm, user, history);
                if let Some(t) = &mut hm.turn_tools {
                    // Inject search_capabilities into turn definitions if discovery is enabled
                    // and not already present.
                    t.add_tool(
                        "search_capabilities",
                        "Search the capability catalog for additional tools or skills when the current set is insufficient. Returns capability IDs, names, descriptions, and kinds.",
                        serde_json::json!({
                            "type": "object",
                            "properties": {
                                "query": {
                                    "type": "string",
                                    "description": "Description of the task, tool, or capability needed."
                                },
                                "limit": {
                                    "type": "integer",
                                    "description": "Maximum number of capabilities to discover (default: 3)."
                                }
                            },
                            "required": ["query"],
                            "additionalProperties": false
                        }),
                    );
                    on_event(AgentEvent::CapabilitySelection(t.summary()));
                    offered_override = Some(t.offered_names());
                }
            }
        }

        // Assemble task-aware project intelligence context if assembler is configured
        if let Some(assembler) = self.context_assembler {
            if let Some(hm) = model.as_any_mut().downcast_mut::<crate::model::HttpModel>() {
                let (mut proj_ctx, stats) = assembler.assemble_context(user);

                // If autonomous checkpoint exists, inject the structured EngineeringPlan into working context
                if let Some(cp_lock) = &self.checkpoint {
                    if let Ok(cp) = cp_lock.lock() {
                        let mut plan_md = format!(
                            "\n\n**Autonomous Engineering Plan (Task `{}`):**\nObjective: {}\nSteps:\n",
                            cp.task_id, cp.objective
                        );
                        for s in &cp.plan.steps {
                            let mark = match s.status {
                                crate::autonomous::StepStatus::Completed => "[x]",
                                crate::autonomous::StepStatus::InProgress => "[>]",
                                crate::autonomous::StepStatus::Failed => "[!]",
                                _ => "[ ]",
                            };
                            plan_md.push_str(&format!("  {} Step {}: {}\n", mark, s.step_id, s.description));
                        }
                        if !cp.modified_files.is_empty() {
                            plan_md.push_str(&format!("Modified Files: {}\n", cp.modified_files.join(", ")));
                        }
                        if !cp.verified_files.is_empty() {
                            plan_md.push_str(&format!("Verified Files: {}\n", cp.verified_files.join(", ")));
                        }
                        proj_ctx.push_str(&plan_md);
                    }
                }

                if !proj_ctx.is_empty() {
                    if let Some(t) = &mut hm.turn_tools {
                        t.project_context = proj_ctx;
                    } else {
                        hm.turn_tools = Some(crate::model::TurnTools {
                            catalog: String::new(),
                            defs: Vec::new(),
                            skills: String::new(),
                            project_context: proj_ctx,
                        });
                    }
                    on_event(AgentEvent::CapabilitySelection(format!(
                        "project intelligence: grounded {} components, {} symbols (est {} tokens in {}µs)",
                        stats.components_matched, stats.symbols_included, stats.estimated_tokens, stats.latency_us
                    )));
                }
            }
        }

        let mut tool_names: Vec<String> = match &offered_override {
            Some(names) => names.clone(),
            None => self.gateway.tools.names(),
        };

        // Doom-loop guard: if the model emits the exact same batch of tool calls
        // (names + args, in order) on `doom_limit` consecutive steps, it is stuck
        // repeating itself with no progress — stop rather than burn the budget.
        let mut last_sig: Option<String> = None;
        let mut repeat_run = 0usize;
        // Denial-streak guard (see `denial_limit`): consecutive steps in which at
        // least one call was refused by policy.
        let mut denial_run = 0usize;

        // Turn-scoped discovery state tracking on-demand capability expansion
        let mut discovery_calls = 0usize;
        const MAX_DISCOVERY_CALLS: usize = 3;
        const MAX_SELECTED_CAPABILITIES: usize = 12;

        // Bounded stall recovery counter: allows at most 2 automatic recovery attempts per turn
        let mut stall_retries = 0usize;
        const MAX_STALL_RETRIES: usize = 2;

        for step in 0..self.max_steps {
            let tools: Vec<&str> = tool_names.iter().map(|s| s.as_str()).collect();
            on_event(AgentEvent::Thinking { step });
            let mut action = if stream {
                let hm = model
                    .as_any_mut()
                    .downcast_mut::<crate::model::HttpModel>()
                    .unwrap();
                let on_ev = std::cell::RefCell::new(&mut on_event);
                let mut sink = |d: &str| (on_ev.borrow_mut())(AgentEvent::TextDelta(d.to_string()));
                let mut reasoning_sink =
                    |r: &str| (on_ev.borrow_mut())(AgentEvent::ReasoningDelta(r.to_string()));
                let res = hm.next_step_streaming(history, &tools, &mut sink, &mut reasoning_sink);
                if let Some(u) = &hm.last_usage {
                    on_event(AgentEvent::Usage(u.clone()));
                }
                res
            } else {
                model.next_step(history, &tools)
            };

            // Non-streaming and mock model repetition inspection
            if let Step::Final(ans) = &action {
                if !ans.contains("[generation stall detected:") {
                    let mut rep = crate::repetition::RepetitionDetector::default();
                    if let Some(phrase) = rep.push_delta(ans) {
                        action = Step::Final(format!("[generation stall detected: repeating phrase \"{phrase}\"]"));
                    }
                }
            }
            match action {
                Step::Final(answer) => {
                    if answer.contains("[generation stall detected:") {
                        let phrase = answer
                            .split("repeating phrase \"")
                            .nth(1)
                            .and_then(|s| s.strip_suffix("\"]"))
                            .unwrap_or("unknown")
                            .to_string();
                        on_event(AgentEvent::StallDetected { phrase: phrase.clone(), step });

                        if stall_retries < MAX_STALL_RETRIES {
                            stall_retries += 1;
                            // Inject corrective operational feedback instead of abruptly stopping
                            history.push(Message::text("assistant", format!("[generation paused: repeating '{phrase}']")));
                            history.push(Message::text(
                                "user",
                                format!(
                                    "A generation stall was detected while repeating '{phrase}'. \
                                     Do not manually enumerate or transcribe items in your output. \
                                     Call a deterministic tool (such as bash, grep, or list_dir with offset/limit) \
                                     to inspect or compute the answer directly."
                                ),
                            ));
                            continue;
                        }
                    }
                    on_event(AgentEvent::StepCompleted {
                        step,
                        summary: format!("final response: {}", answer.chars().take(80).collect::<String>()),
                    });
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
                    let call_count = calls.len();
                    for call in calls {
                        on_event(AgentEvent::ToolStart { name: call.name.clone(), args: call.args.clone() });
                        history.push(Message {
                            role: "assistant".into(),
                            content: format!("→ {}({})", call.name, call.args),
                            tool_call: Some(call.clone()),
                        });
                        let outcome = if call.name == "search_capabilities" {
                            // Intercept search_capabilities if not in gateway, or execute via selector
                            if let Some(sel) = self.selector {
                                let query = call.args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                                let limit = call.args.get("limit").and_then(|v| v.as_u64()).map(|u| u as usize).unwrap_or(3);
                                if discovery_calls >= MAX_DISCOVERY_CALLS {
                                    Ok(tools::Output::ok(format!(
                                        "Discovery limit reached: maximum {MAX_DISCOVERY_CALLS} calls per turn allowed."
                                    )))
                                } else {
                                    discovery_calls += 1;
                                    let start = std::time::Instant::now();
                                    let tool_reg = sel.tool_registry();
                                    let skill_reg = sel.skill_registry();

                                    let tool_hits = tool_reg
                                        .search_with_options(query, limit, onyx_retrieval::Origin::Agent, onyx_retrieval::SearchOptions::new(onyx_retrieval::SearchMethod::Bm25))
                                        .unwrap_or_default();
                                    let skill_hits = skill_reg
                                        .search_with_options(query, limit, onyx_retrieval::Origin::Agent, onyx_retrieval::SearchOptions::new(onyx_retrieval::SearchMethod::Bm25))
                                        .unwrap_or_default();

                                    let mut results = Vec::new();
                                    let mut mounted_tools = Vec::new();

                                    // Mount newly discovered tools into this turn's active definitions and tools list
                                    for hit in tool_hits {
                                        if let Some(tool) = tool_reg.get(&hit.tool_id) {
                                            // Progressive disclosure & quota check
                                            if tool_names.len() < MAX_SELECTED_CAPABILITIES {
                                                if !tool_names.contains(&tool.name) {
                                                    tool_names.push(tool.name.clone());
                                                    mounted_tools.push(tool.name.clone());
                                                }
                                                if let Some(hm) = model.as_any_mut().downcast_mut::<crate::model::HttpModel>() {
                                                    if let Some(t) = &mut hm.turn_tools {
                                                        t.add_tool(&tool.name, &tool.description, tool.input_schema.clone());
                                                    }
                                                }
                                            }
                                            results.push(serde_json::json!({
                                                "id": tool.id,
                                                "name": tool.name,
                                                "description": tool.description,
                                                "kind": "tool",
                                                "score": hit.score,
                                            }));
                                        }
                                    }

                                    // L0 discovery for skills
                                    for hit in skill_hits {
                                        if let Some(skill) = skill_reg.get(&hit.skill_id) {
                                            results.push(serde_json::json!({
                                                "id": skill.id,
                                                "name": skill.name,
                                                "description": skill.description,
                                                "kind": "skill",
                                                "score": hit.score,
                                            }));
                                        }
                                    }

                                    let took_ms = start.elapsed().as_millis() as u64;
                                    sel.tool_sink().record(onyx_retrieval::TraceEvent::GatewaySearch {
                                        query: query.to_string(),
                                        origin: onyx_retrieval::Origin::Agent,
                                        top_k: limit as u32,
                                        hits: results.len() as u32,
                                        took_ms,
                                    });

                                    let out_str = serde_json::to_string_pretty(&serde_json::json!({
                                        "capabilities": results,
                                        "mounted_tools": mounted_tools,
                                        "message": "Discovered capabilities. Any mounted tools are immediately available for invocation."
                                    })).unwrap_or_else(|_| "{}".to_string());
                                    Ok(tools::Output::ok(out_str))
                                }
                            } else {
                                self.gateway.call(&call.name, &call.args)
                            }
                        } else {
                            self.gateway.call(&call.name, &call.args)
                        };
                        // Feed real usage to the retrieval learner so adaptive
                        // ranking credits the search that offered this tool. No-op
                        // when the usage arm is off or no selector is attached.
                        if let Some(sel) = self.selector {
                            let (ok, err) = match &outcome {
                                Ok(out) => (!out.is_error, String::new()),
                                Err(e) => (false, e.to_string()),
                            };
                            sel.record_invoke(&call.name, &call.args, ok, 0, &err);
                        }
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
                        let mut content = match &outcome {
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
                            Err(e) => {
                                // Feed the tool execution error back to the model as a tool output
                                // so the agent can understand the error (e.g. file not found, bad args)
                                // and recover/retry or inspect another path, rather than crashing the loop.
                                format!("ERROR: {e}")
                            }
                        };

                        // Section 1: Failure Diagnostic Interceptor
                        // Check if outcome failed (either Gateway returned Err or bash command exited non-zero with is_error=true)
                        let is_failure = match &outcome {
                            Ok(out) => out.is_error,
                            Err(_) => true,
                        };

                        if is_failure {
                            let failed_cmd = call.args.get("command")
                                .or_else(|| call.args.get("cmd"))
                                .and_then(|v| v.as_str())
                                .unwrap_or(&call.name);

                            let error_text = match &outcome {
                                Ok(out) => out.raw_content.as_deref().unwrap_or(&out.content),
                                Err(_e) => &content,
                            };

                            let (arch_ref, root) = if let Some(assembler) = self.context_assembler {
                                (Some(assembler.get_arch_ref()), assembler.workspace_root.clone())
                            } else {
                                (None, std::path::PathBuf::from("."))
                            };

                            if let Some(arch) = arch_ref {
                                let engine = crate::autonomous::AutonomousEngine::new(&arch, &root);
                                let retry_count = if let Some(cp_lock) = &self.checkpoint {
                                    cp_lock.lock().map(|cp| {
                                        cp.current_diagnostics.as_ref().map(|d| d.retry_count + 1).unwrap_or(1)
                                    }).unwrap_or(1)
                                } else {
                                    1
                                };

                                let observation = match &outcome {
                                    Ok(out) => out.observation.as_ref(),
                                    Err(_) => None,
                                };
                                let focus_files = self
                                    .checkpoint
                                    .as_ref()
                                    .and_then(|cp| cp.lock().ok())
                                    .map(|cp| cp.modified_files.clone());
                                let diag = engine.diagnose_failure_with_observation_and_focus(
                                    failed_cmd,
                                    error_text,
                                    retry_count,
                                    observation,
                                    focus_files.as_deref(),
                                );
                                content.push_str(&format!(
                                    "\n\n[Autonomous Diagnostic: {:?} | Location: {:?}:{} | Suggestion: {}]",
                                    diag.error_message,
                                    diag.error_file,
                                    diag.error_line.unwrap_or(0),
                                    diag.suggested_remedy
                                ));

                                if let Some(cp_lock) = &self.checkpoint {
                                    if let Ok(mut cp) = cp_lock.lock() {
                                        cp.current_diagnostics = Some(diag);
                                        let _ = cp.save_atomic(&root);
                                    }
                                }
                            }
                        } else {
                            // Section 2: Post-Edit Test Selection & Checkpoint update on modification
                            let is_modifying_tool = call.name == "editor" || call.name == "write_file" || call.name == "search_replace";
                            if is_modifying_tool {
                                let modified_file = call.args.get("path")
                                    .or_else(|| call.args.get("file"))
                                    .and_then(|v| v.as_str());

                                if let Some(file_path) = modified_file {
                                    let (arch_ref, root) = if let Some(assembler) = self.context_assembler {
                                        assembler.invalidate_cache();
                                        (Some(assembler.get_arch_ref()), assembler.workspace_root.clone())
                                    } else {
                                        (None, std::path::PathBuf::from("."))
                                    };

                                    if let Some(arch) = arch_ref {
                                        let engine = crate::autonomous::AutonomousEngine::new(&arch, &root);
                                        let selected_tests = engine.select_tests_for_changes(&[file_path.to_string()]);

                                        if !selected_tests.is_empty() {
                                            content.push_str(&format!(
                                                "\n\n[Post-Edit Verification Plan: Modified `{}` affects tests: {}]",
                                                file_path,
                                                selected_tests.join(", ")
                                            ));
                                        }

                                        if let Some(cp_lock) = &self.checkpoint {
                                            if let Ok(mut cp) = cp_lock.lock() {
                                                if !cp.modified_files.contains(&file_path.to_string()) {
                                                    cp.modified_files.push(file_path.to_string());
                                                }
                                                cp.record_file_hash(&root, file_path);
                                                for t in &selected_tests {
                                                    if !cp.plan.targeted_tests.contains(t) {
                                                        cp.plan.targeted_tests.push(t.clone());
                                                    }
                                                }
                                                let _ = cp.save_atomic(&root);
                                            }
                                        }

                                        // Section 2b: Update CoverageTracker & WorkMemory
                                        let mut cov = onyx_project::CoverageTracker::load_or_init(&root);
                                        cov.record_modification(file_path, None, None, Some(format!("modified via {}", call.name)));
                                        let _ = cov.save_atomic(&root);

                                        let mut mem = onyx_project::WorkMemory::load_or_init(&root);
                                        let mut summary = onyx_project::TaskSummary::new(
                                            format!("mod-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()),
                                            format!("Modified `{}` via {}", file_path, call.name),
                                        );
                                        summary.files_modified.push(file_path.to_string());
                                        summary.successful_changes.push(format!("Tool {} applied edits to {}", call.name, file_path));
                                        mem.record_task_summary(summary);
                                        let _ = mem.save_atomic(&root);
                                    }
                                }
                            } else if call.name == "read_file" {
                                let read_path = call.args.get("path")
                                    .or_else(|| call.args.get("file"))
                                    .and_then(|v| v.as_str());
                                if let Some(path_str) = read_path {
                                    let root = if let Some(assembler) = self.context_assembler {
                                        assembler.workspace_root.clone()
                                    } else {
                                        std::path::PathBuf::from(".")
                                    };
                                    let mut cov = onyx_project::CoverageTracker::load_or_init(&root);
                                    cov.record_inspection(path_str, None, None, Some(format!("inspected via {}", call.name)));
                                    let _ = cov.save_atomic(&root);
                                }
                            } else if call.name == "grep" {
                                let query = call.args.get("query")
                                    .or_else(|| call.args.get("pattern"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let root = if let Some(assembler) = self.context_assembler {
                                    assembler.workspace_root.clone()
                                } else {
                                    std::path::PathBuf::from(".")
                                };
                                let mut cov = onyx_project::CoverageTracker::load_or_init(&root);
                                // If grep output mentions files, record search hits
                                for line in content.lines() {
                                    if let Some((file, _)) = line.split_once(':') {
                                        if !file.contains(' ') && (file.contains('/') || file.ends_with(".rs") || file.ends_with(".ts") || file.ends_with(".js") || file.ends_with(".json")) {
                                            cov.record_search_hit(file.trim(), None, query);
                                        }
                                    }
                                }
                                let _ = cov.save_atomic(&root);
                            } else if call.name == "bash" {
                                // Record verification result if running tests or cargo check
                                let cmd = call.args.get("command").or_else(|| call.args.get("cmd")).and_then(|v| v.as_str()).unwrap_or("");
                                let root = if let Some(assembler) = self.context_assembler {
                                    assembler.workspace_root.clone()
                                } else {
                                    std::path::PathBuf::from(".")
                                };

                                let is_test_run = cmd.contains("test") || cmd.contains("pytest") || cmd.contains("cargo test") || cmd.contains("jest") || cmd.contains("vitest");
                                let is_build_run = cmd.contains("cargo check") || cmd.contains("cargo build") || cmd.contains("tsc") || cmd.contains("build");

                                if is_test_run || is_build_run {
                                    let affected: Vec<String> = if let Some(cp_lock) = &self.checkpoint {
                                        cp_lock.lock().map(|cp| cp.modified_files.clone()).unwrap_or_default()
                                    } else {
                                        Vec::new()
                                    };

                                    let mut cov = onyx_project::CoverageTracker::load_or_init(&root);
                                    if is_test_run {
                                        cov.record_test_run(
                                            cmd,
                                            None,
                                            !is_failure,
                                            &affected,
                                            None,
                                            onyx_project::VerificationScope::WorkspaceSuite,
                                            Some(format!("Executed via bash: {cmd}")),
                                        );
                                    } else if is_build_run {
                                        cov.record_runtime_verification(
                                            cmd,
                                            !is_failure,
                                            &affected,
                                            None,
                                            onyx_project::VerificationScope::WorkspaceSuite,
                                            Some(format!("Build verification via bash: {cmd}")),
                                        );
                                    }
                                    let _ = cov.save_atomic(&root);
                                }

                                if (cmd.contains("cargo test") || cmd.contains("cargo check")) && !is_failure {
                                    if let Some(cp_lock) = &self.checkpoint {
                                        if let Ok(mut cp) = cp_lock.lock() {
                                            if !cp.verification_results.contains(&cmd.to_string()) {
                                                cp.verification_results.push(cmd.to_string());
                                            }
                                            let _ = cp.save_atomic(&root);
                                        }
                                    }
                                }
                            }
                        }
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
                        on_event(AgentEvent::StepCompleted {
                            step,
                            summary: format!("terminated with answer: {}", answer.chars().take(80).collect::<String>()),
                        });
                        return Ok(RunReport { answer, steps: step + 1, history: history.clone() });
                    }
                    on_event(AgentEvent::StepCompleted {
                        step,
                        summary: format!("completed {} tool calls", call_count),
                    });
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
