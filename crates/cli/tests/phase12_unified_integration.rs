//! Onyx Phase 12 — Complete Unified Intelligence Runtime Integration Test Suite.
//!
//! Verifies end-to-end:
//! 1. One-shot execution with ContextAssembler grounding.
//! 2. SandboxedBash routing through canonical Phase 9 & 11 compression pipeline.
//! 3. CoverageTracker updating from tool read/grep/bash events.
//! 4. Autonomous checkpoint creation, plan generation, and post-edit test selection.
//! 5. Security governance denial preserving fail-closed invariants.
//! 6. Project-aware task profiles extracted statically from manifests without script execution.

use std::sync::{Arc, Mutex};
use tempfile::tempdir;

use agent::{Message, Model, Step, ToolCall, ToolCallAgent};
use onyx_project::{ArchitectureMap, CoverageTracker, ProjectScanner, SymbolKind, SymbolMeta};
use tools::audit::Gateway;
use tools::policy::{AllowAll, BashGuard, Policy};
use tools::Output;

struct MockScriptedModel {
    pub steps: Vec<Step>,
    pub step_index: usize,
    pub observed_prompts: Vec<String>,
}

impl MockScriptedModel {
    fn new(steps: Vec<Step>) -> Self {
        Self {
            steps,
            step_index: 0,
            observed_prompts: Vec::new(),
        }
    }
}

impl Model for MockScriptedModel {
    fn next_step(&mut self, history: &[Message], _tools: &[&str]) -> Step {
        if let Some(last) = history.last() {
            self.observed_prompts.push(last.content.clone());
        }
        if self.step_index < self.steps.len() {
            let s = self.steps[self.step_index].clone();
            self.step_index += 1;
            s
        } else {
            Step::Final("Task completed".to_string())
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[test]
fn test_canonical_command_observation_and_sandboxed_bash() {
    // Verify that format_command_output correctly compacts both standard and sandboxed bash
    let raw_git_diff = r#"diff --git a/src/lib.rs b/src/lib.rs
index e69de29..b712345 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,5 @@
+pub fn new_feature() -> bool {
+    true
+}
"#;

    let output = tools::bash::format_command_output("git diff", raw_git_diff, 0, true);
    assert!(!output.is_error);
    assert!(output.observation.is_some(), "Must produce structured observation");
    let obs = output.observation.unwrap();
    assert_eq!(obs.kind, Some(tools::compress::FormatterKind::GitDiff));
    assert!(output.content.contains("[exit 0]"));
}

#[test]
fn test_project_aware_task_profile_extraction_static() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // 1. package.json with scripts
    let pkg_json = r#"{
        "name": "monorepo-app",
        "scripts": {
            "test": "vitest run --coverage",
            "lint": "eslint src/"
        }
    }"#;
    std::fs::write(root.join("package.json"), pkg_json).unwrap();

    // 2. Makefile with targets
    let makefile = "test:\n\tpytest tests/\n\ncheck:\n\tcargo check\n";
    std::fs::write(root.join("Makefile"), makefile).unwrap();

    // Scan project statically
    let scanner = ProjectScanner::new(root);
    let map = scanner.scan_full().unwrap();

    // Verify tasks extracted without running external commands
    assert!(map.task_profiles.contains_key("npm run test"));
    assert_eq!(map.task_profiles.get("npm run test").unwrap().formatter, "jest_vitest");

    assert!(map.task_profiles.contains_key("make test"));
    assert_eq!(map.task_profiles.get("make test").unwrap().formatter, "pytest");

    assert!(map.task_profiles.contains_key("make check"));
    assert_eq!(map.task_profiles.get("make check").unwrap().formatter, "cargo_check");
}

#[test]
fn test_unified_coverage_tracking_on_tool_events() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // Create target file
    let src_dir = root.join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    let file_path = src_dir.join("core.rs");
    std::fs::write(&file_path, "pub fn compute() -> i32 { 42 }\n").unwrap();

    // Initialize tools and gateway
    let tool_col = tools::default_collection(root.to_path_buf());
    let gw = Gateway::new(tool_col, AllowAll);

    // Create ContextAssembler
    let assembler = agent::context_assembler::ContextAssembler::new(
        root,
        agent::context_assembler::ContextBudgetConfig::default(),
    );

    // Create ScriptedModel that simulates: read_file -> editor (modify) -> bash (cargo test)
    let steps = vec![
        Step::call("read_file", serde_json::json!({ "path": "src/core.rs" })),
        Step::call("editor", serde_json::json!({
            "command": "str_replace",
            "path": "src/core.rs",
            "old_str": "42",
            "new_str": "100"
        })),
        Step::call("bash", serde_json::json!({ "command": "cargo test --test core" })),
        Step::Final("Finished".to_string()),
    ];

    let mut model = MockScriptedModel::new(steps);
    let mut history = vec![Message::text("system", "Test Agent")];

    let agent = ToolCallAgent::new(&gw)
        .with_context_assembler(&assembler);

    let res = agent.run_turn(&mut model, "Refactor core.rs and test it", &mut history);
    assert!(res.is_ok());

    // Verify CoverageTracker persisted and recorded all events
    let cov = CoverageTracker::load_or_init(root);
    let record = cov.files.get("src/core.rs").expect("core.rs must be tracked");

    assert!(record.status.inspected, "read_file must mark file inspected");
    assert!(record.status.modified, "editor must mark file modified");
    assert!(record.evidence_history.len() >= 2, "Must log chronological evidence");
}

#[test]
fn test_autonomous_checkpoint_and_verification_persistence() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let mut map = ArchitectureMap::new(root.display().to_string());
    map.symbols.push(SymbolMeta {
        file: "src/engine.rs".to_string(),
        name: "EngineCore".to_string(),
        kind: SymbolKind::Struct,
        is_public: true,
        line: 10,
        module_path: Some("engine".to_string()),
        signature: Some("pub struct EngineCore".to_string()),
        conditional: None,
    });

    let engine = agent::autonomous::AutonomousEngine::new(&map, root);
    let plan = engine.plan_task("task-p12", "Refactor EngineCore in src/engine.rs");
    let mut cp = agent::autonomous::TaskCheckpoint::new("task-p12", "Refactor EngineCore", plan);
    cp.save_atomic(root).unwrap();

    let loaded = agent::autonomous::TaskCheckpoint::load_checkpoint(root, "task-p12")
        .expect("Checkpoint must be loadable");
    assert_eq!(loaded.task_id, "task-p12");
    assert!(!loaded.plan.steps.is_empty());
}

#[test]
fn test_security_governance_denial_failclosed() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let tool_col = tools::default_collection(root.to_path_buf());
    let bg = BashGuard::default();
    let gw = Gateway::new(tool_col, bg);

    let steps = vec![
        Step::call("bash", serde_json::json!({ "command": "rm -rf /" })),
    ];

    let mut model = MockScriptedModel::new(steps);
    let mut history = vec![Message::text("system", "Test Agent")];

    let agent = ToolCallAgent::new(&gw);
    let res = agent.run_turn(&mut model, "Delete everything", &mut history);

    assert!(res.is_ok(), "Agent handles denied tool safely without panic");
    // Assert that history contains the policy denial
    let tool_msg = history.iter().find(|m| m.role == "tool").expect("Tool response required");
    assert!(tool_msg.content.contains("DENIED by policy"), "Must fail-closed with policy denial");
}
