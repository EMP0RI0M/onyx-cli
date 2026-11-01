//! subagent — thread fan-out over `agent::ToolCallAgent` for Onyx.
//!
//! Design:
//! - Spawns child agent threads using `std::thread::spawn` (blocking, no async/tokio).
//! - Depth guard: child subagents are denied access to `spawn_subagent` and `swarm`
//!   tools to prevent unbounded recursive fork-bombs.
//! - Audit log stitching: child subagents run with their own Gateway, and when
//!   they finish their execution, their audit trail is returned and can be merged upward.
//! - Bounded concurrency: `swarm` enforces bounded fan-out (max 2-3 workers concurrently)
//!   to honor memory constraints under PRoot on Android.
//!
//! Thread-safety: a `ToolCollection` clone shares each tool through an `Arc`, so
//! workers move their own snapshot onto their thread; the live parent collection
//! is never borrowed across threads.

use agent::{AgentError, Model, ToolCallAgent};
use std::sync::{Arc, Mutex};
use std::thread;
use tools::audit::{AuditEntry, Gateway};
use tools::policy::AllowAll;
use tools::{Args, Output, Tool, ToolCollection, ToolError};

/// Tools stripped from every child for the depth guard.
const SPAWN_TOOLS: [&str; 2] = ["spawn_subagent", "swarm"];

/// Subagent execution outcome.
#[derive(Debug, Clone)]
pub struct SubagentResult {
    pub task: String,
    pub answer: String,
    pub steps: usize,
    pub audit: Vec<AuditEntry>,
}

fn build_child_tools(
    parent_tools: &ToolCollection,
    tools_subset: Option<&[&str]>,
) -> ToolCollection {
    let mut child_tools = parent_tools.clone();
    if let Some(subset) = tools_subset {
        child_tools.retain(subset);
    }
    // Depth guard: remove any subagent spawning capability in child
    let allowed: Vec<String> = child_tools
        .names()
        .into_iter()
        .filter(|n| !SPAWN_TOOLS.contains(&n.as_str()))
        .collect();
    let allowed_refs: Vec<&str> = allowed.iter().map(|s| s.as_str()).collect();
    child_tools.retain(&allowed_refs);
    child_tools
}

fn run_child<M: Model + Send + 'static>(
    model: &mut M,
    tools: ToolCollection,
    task: &str,
    max_steps: usize,
) -> Result<SubagentResult, AgentError> {
    let gw = Gateway::new(tools, AllowAll);
    let mut agent = ToolCallAgent::new(&gw);
    agent.max_steps = max_steps;
    agent.system = "You are an onyx subagent dedicated to a single subtask. \
                    Complete the task and call terminate with your findings."
        .to_string();
    let rep = agent.run(model, task)?;
    Ok(SubagentResult {
        task: task.to_string(),
        answer: rep.answer,
        steps: rep.steps,
        audit: gw.audit.snapshot(),
    })
}

/// Spawns a subagent on a dedicated OS thread and blocks waiting for completion.
///
/// `tools_subset`: List of tool names to retain from the parent collection.
/// To protect against unbounded recursion, `spawn_subagent` and `swarm` are ALWAYS
/// stripped from the child's tool collection.
pub fn spawn_subagent<M: Model + Send + 'static>(
    task: &str,
    mut model: M,
    parent_tools: &ToolCollection,
    tools_subset: Option<&[&str]>,
    max_steps: usize,
) -> Result<SubagentResult, AgentError> {
    let child_tools = build_child_tools(parent_tools, tools_subset);
    let task_owned = task.to_string();
    let handle = thread::spawn(move || run_child(&mut model, child_tools, &task_owned, max_steps));

    handle.join().unwrap_or_else(|_| {
        Err(AgentError::Tool(ToolError::Other(
            "subagent thread panicked".into(),
        )))
    })
}

/// Bounded worker pool: runs multiple subagent tasks with bounded concurrency (default 2-3).
pub fn swarm<M: Model + Clone + Send + 'static>(
    tasks: &[String],
    model: M,
    parent_tools: &ToolCollection,
    tools_subset: Option<&[&str]>,
    max_steps: usize,
    concurrency_limit: usize,
) -> Vec<Result<SubagentResult, AgentError>> {
    let limit = concurrency_limit.clamp(1, 4);
    let child_tools = Arc::new(build_child_tools(parent_tools, tools_subset));
    let queue = Arc::new(Mutex::new(
        tasks
            .iter()
            .cloned()
            .enumerate()
            .collect::<std::collections::VecDeque<_>>(),
    ));
    let results = Arc::new(Mutex::new(Vec::with_capacity(tasks.len())));

    let mut handles = Vec::with_capacity(limit);
    for _ in 0..limit {
        let q = Arc::clone(&queue);
        let res = Arc::clone(&results);
        let tools = Arc::clone(&child_tools);
        let mut m = model.clone();

        handles.push(thread::spawn(move || loop {
            let item = {
                let mut lock = q.lock().unwrap();
                lock.pop_front()
            };
            let (idx, task) = match item {
                Some(pair) => pair,
                None => break,
            };
            let outcome = run_child(&mut m, (*tools).clone(), &task, max_steps);
            let mut lock = res.lock().unwrap();
            lock.push((idx, outcome));
        }));
    }

    for h in handles {
        let _ = h.join();
    }

    let mut lock = results.lock().unwrap();
    lock.sort_by_key(|(idx, _)| *idx);
    lock.drain(..).map(|(_, r)| r).collect()
}

/// Tool implementation: `spawn_subagent`
pub struct SpawnSubagentTool<M: Model + Clone + Send + 'static> {
    pub model: M,
    pub tools: ToolCollection,
    pub max_steps: usize,
}

impl<M: Model + Clone + Send + 'static> SpawnSubagentTool<M> {
    pub fn new(model: M, tools: ToolCollection, max_steps: usize) -> Self {
        Self { model, tools, max_steps }
    }
}

impl<M: Model + Clone + Send + 'static> Tool for SpawnSubagentTool<M> {
    fn name(&self) -> &str {
        "spawn_subagent"
    }

    fn description(&self) -> &str {
        "Spawn a subagent to independently investigate or solve a specific task. Args: {\"task\": \"...\", \"tools\": [\"bash\", ...]} (tools optional)."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec!["task".into()]
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let task = args
            .get("task")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::BadArgs("missing `task` string".into()))?;

        let subset_storage: Option<Vec<String>> = args.get("tools").and_then(|t| t.as_array()).map(|arr| {
            arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()
        });
        let subset_refs: Option<Vec<&str>> = subset_storage.as_ref().map(|v| v.iter().map(|s| s.as_str()).collect());

        match spawn_subagent(task, self.model.clone(), &self.tools, subset_refs.as_deref(), self.max_steps) {
            Ok(res) => Ok(Output::ok(format!(
                "Subagent completed task: {}\nAnswer:\n{}\n[steps: {}, sub-actions: {}]",
                res.task,
                res.answer,
                res.steps,
                res.audit.len()
            ))),
            Err(e) => Ok(Output::err(format!("Subagent failed: {e:?}"))),
        }
    }
}

/// Tool implementation: `swarm` (parallel subagents)
pub struct SwarmTool<M: Model + Clone + Send + 'static> {
    pub model: M,
    pub tools: ToolCollection,
    pub max_steps: usize,
    pub max_workers: usize,
}

impl<M: Model + Clone + Send + 'static> SwarmTool<M> {
    pub fn new(model: M, tools: ToolCollection, max_steps: usize, max_workers: usize) -> Self {
        Self {
            model,
            tools,
            max_steps,
            max_workers,
        }
    }
}

impl<M: Model + Clone + Send + 'static> Tool for SwarmTool<M> {
    fn name(&self) -> &str {
        "swarm"
    }

    fn description(&self) -> &str {
        "Run multiple subagents concurrently on a list of independent subtasks. Args: {\"tasks\": [\"task 1\", \"task 2\"], \"tools\": [\"bash\", ...]} (tools optional)."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec!["tasks".into()]
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let tasks_arr = args
            .get("tasks")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ToolError::BadArgs("missing `tasks` array".into()))?;

        let tasks: Vec<String> = tasks_arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();

        if tasks.is_empty() {
            return Err(ToolError::BadArgs("tasks array is empty".into()));
        }

        let subset_storage: Option<Vec<String>> = args.get("tools").and_then(|t| t.as_array()).map(|arr| {
            arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()
        });
        let subset_refs: Option<Vec<&str>> = subset_storage.as_ref().map(|v| v.iter().map(|s| s.as_str()).collect());

        let results = swarm(
            &tasks,
            self.model.clone(),
            &self.tools,
            subset_refs.as_deref(),
            self.max_steps,
            self.max_workers,
        );

        let mut out = String::new();
        out.push_str(&format!("Swarm completed {} task(s):\n", results.len()));
        for (i, res) in results.into_iter().enumerate() {
            match res {
                Ok(r) => {
                    out.push_str(&format!("\n--- [Worker {}] {} ---\n{}\n", i + 1, r.task, r.answer));
                }
                Err(e) => {
                    out.push_str(&format!("\n--- [Worker {}] FAILED ---\n{:?}\n", i + 1, e));
                }
            }
        }
        Ok(Output::ok(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent::{run_scripted_entry, Message, Step, ToolCall};
    use serde_json::json;

    /// Replays a fixed script; once exhausted it terminates with `answer`.
    /// Shared state lets one instance drive several worker threads at once.
    #[derive(Clone)]
    struct ScriptedModel {
        steps: Arc<Mutex<std::collections::VecDeque<Step>>>,
        answer: String,
    }

    impl ScriptedModel {
        fn new(steps: Vec<Step>, answer: &str) -> Self {
            Self {
                steps: Arc::new(Mutex::new(steps.into())),
                answer: answer.to_string(),
            }
        }
    }

    impl Model for ScriptedModel {
        fn next_step(&mut self, _history: &[Message], _tools: &[&str]) -> Step {
            let next = {
                let mut lock = self.steps.lock().unwrap();
                lock.pop_front()
            };
            next.unwrap_or_else(|| Step::Final(self.answer.clone()))
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }

    struct SimpleEcho;
    impl Tool for SimpleEcho {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "echo"
        }
        fn execute(&self, args: &Args) -> Result<Output, ToolError> {
            let msg = args.get("msg").and_then(|v| v.as_str()).unwrap_or("");
            Ok(Output::ok(msg.to_string()))
        }
    }

    fn base_tools() -> ToolCollection {
        let mut tools = ToolCollection::new();
        tools.add(SimpleEcho);
        tools.add(tools::control::Terminate);
        tools
    }

    #[test]
    fn subagent_executes_task_and_records_audit() {
        let tools = base_tools();
        let model = ScriptedModel::new(
            vec![
                Step::call("echo", json!({"msg": "hello from child"})),
                Step::call("terminate", json!({"status": "ok", "content": "finished task"})),
            ],
            "unused",
        );

        let res = spawn_subagent("do echo", model, &tools, None, 5).unwrap();
        assert_eq!(res.answer, "finished task");
        assert_eq!(res.steps, 2);
        assert_eq!(res.audit.len(), 2);
        assert_eq!(res.audit[0].tool, "echo");
        assert_eq!(res.audit[1].tool, "terminate");
    }

    #[test]
    fn subagent_depth_guard_strips_spawn_tools() {
        let mut tools = base_tools();
        // Add dummy spawn_subagent and swarm in parent collection
        let dummy_model = ScriptedModel::new(vec![], "never");
        tools.add(SpawnSubagentTool::new(dummy_model.clone(), ToolCollection::new(), 4));
        tools.add(SwarmTool::new(dummy_model, ToolCollection::new(), 4, 2));

        assert!(tools.get("spawn_subagent").is_some());
        assert!(tools.get("swarm").is_some());

        // Now spawn subagent and check that within the subagent, spawn_subagent cannot be called
        let model = ScriptedModel::new(
            vec![
                Step::call("spawn_subagent", json!({"task": "recursive"})),
                Step::call("terminate", json!({"content": "never reached"})),
            ],
            "never reached either",
        );

        let res = spawn_subagent("test depth", model, &tools, None, 4);
        assert!(res.is_err(), "Calling spawn_subagent from child must fail at gateway");
    }

    #[test]
    fn swarm_executes_multiple_tasks_bounded() {
        let tools = base_tools();
        let model = ScriptedModel::new(
            vec![
                Step::call("echo", json!({"msg": "work done"})),
                Step::call("terminate", json!({"content": "all good"})),
            ],
            "all good",
        );

        let tasks = vec!["task A".to_string(), "task B".to_string(), "task C".to_string()];
        let results = swarm(&tasks, model, &tools, None, 5, 2);
        assert_eq!(results.len(), 3);
        for res in results {
            let r = res.unwrap();
            assert_eq!(r.answer, "all good");
        }
    }

    #[test]
    fn scripted_entry_runs_through_gateway_with_audit() {
        let gw = Gateway::new(base_tools(), AllowAll);
        let steps = vec![
            Step::call("echo", json!({"msg": "hi"})),
            Step::call("terminate", json!({"content": "entry done"})),
        ];
        let (rep, audit) = run_scripted_entry(&gw, steps, "scripted entry", 4).unwrap();
        assert_eq!(rep.answer, "entry done");
        assert_eq!(audit.len(), 2);
        assert_eq!(audit[0].tool, "echo");
    }
}
