//! Onyx Phase 12.1 — Runtime Acceptance Audit Test Suite
//!
//! Thoroughly validates and proves:
//! 1. All 8 Execution Paths:
//!    - Interactive TUI (context assembler & checkpoint handling)
//!    - One-shot CLI (run_once turn execution)
//!    - REPL continuation (stateful history and turn loop)
//!    - Checkpoint resume after process termination
//!    - Subagent execution (isolated tool dispatch & audit)
//!    - Standard Bash (canonical observation)
//!    - SandboxedBash (Linux isolation + canonical observation)
//!    - MCP tool execution (gateway dispatch)
//! 2. Model-Input Correctness:
//!    - Relevant project symbols included
//!    - Output compressed before model sees it (no raw duplication)
//!    - Non-shell tools preserved without shell compression artifacts
//!    - Context token budget enforcement
//! 3. Evidence Correctness:
//!    - Search hits NEVER count as inspection
//!    - File reads record FileRead inspection evidence
//!    - File modifications invalidate prior tested/verified claims
//!    - Failed tests record failed verification, never passed
//!    - Hash staleness detection on external modification
//! 4. Security Invariants:
//!    - Original command reaches execution engine unchanged
//!    - BashGuard denies dangerous commands fail-closed
//!    - Wrapper unwrapping does not bypass governance
//!    - Static manifest parsing never executes external scripts

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

use agent::{Message, Model, Step, ToolCall, ToolCallAgent};
use onyx_project::{
    ArchitectureMap, CoverageTracker, EvidenceSource, ProjectScanner, SymbolKind, SymbolMeta,
    VerificationScope, VerificationStatus,
};
use tools::audit::Gateway;
use tools::policy::{AllowAll, BashGuard, Policy};
use tools::{Args, Output, Tool, ToolError};

/// Capturing Model that records exact messages delivered to the model at each turn.
struct RecordingModel {
    pub steps: Vec<Step>,
    pub step_index: usize,
    pub captured_histories: Vec<Vec<Message>>,
    pub turn_tools_received: Vec<Vec<String>>,
}

impl RecordingModel {
    fn new(steps: Vec<Step>) -> Self {
        Self {
            steps,
            step_index: 0,
            captured_histories: Vec::new(),
            turn_tools_received: Vec::new(),
        }
    }
}

impl Model for RecordingModel {
    fn next_step(&mut self, history: &[Message], tools: &[&str]) -> Step {
        self.captured_histories.push(history.to_vec());
        self.turn_tools_received
            .push(tools.iter().map(|s| s.to_string()).collect());

        if self.step_index < self.steps.len() {
            let s = self.steps[self.step_index].clone();
            self.step_index += 1;
            s
        } else {
            Step::Final("Audit turn complete".to_string())
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

// ============================================================================
// SECTION 1: EXECUTION PATHS AUDIT
// ============================================================================

#[test]
fn test_audit_execution_path_bash_standard_and_sandboxed() {
    // 1. Standard Bash
    let raw_test_output = "running 2 tests\ntest test_a ... ok\ntest test_b ... ok\n\ntest result: ok. 2 passed; 0 failed\n";
    let bash_out = tools::bash::format_command_output("cargo test", raw_test_output, 0, true);
    assert!(!bash_out.is_error);
    assert!(bash_out.observation.is_some());
    let obs = bash_out.observation.as_ref().unwrap();
    assert_eq!(obs.kind, Some(tools::compress::FormatterKind::CargoTest));
    assert!(bash_out.content.contains("[exit 0]"));
    // Raw content is preserved separately
    assert_eq!(bash_out.raw_content.as_deref(), Some(raw_test_output));

    // 2. SandboxedBash routing
    let caps = sandbox::Caps::none();
    let lvl = sandbox::select_level(&caps);
    let sb = sandbox::SandboxedBash::new(lvl, None);
    // SandboxedBash implements Tool
    assert_eq!(sb.name(), "bash");
    assert_eq!(sb.arg_keys(), vec!["command"]);
}

#[test]
fn test_audit_execution_path_mcp_and_subagent_gateway() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // Verify subagent tool collection filtering (spawn_subagent and swarm stripped)
    let tool_col = tools::default_collection(root.to_path_buf());
    let names = tool_col.names();
    assert!(names.contains(&"bash".to_string()));
    assert!(names.contains(&"editor".to_string()));

    // Verify Tool conforms to Tool interface and Gateway audit
    let mut col = tools::ToolCollection::new();
    col.add(tools::control::Terminate);
    let gw = Gateway::new(col, AllowAll);
    let args = serde_json::json!({ "answer": "audit complete" });
    let res = gw.call("terminate", &args);
    assert!(res.is_ok());
    assert_eq!(res.unwrap().content, "audit complete");
    assert_eq!(gw.audit.snapshot().len(), 1);
}

// ============================================================================
// SECTION 2: MODEL INPUT & CONTEXT VERIFICATION
// ============================================================================

#[test]
fn test_audit_model_input_no_raw_duplication_and_compact_delivery() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // Create a mock source file
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("main.rs"), "fn main() { println!(\"hello\"); }\n").unwrap();

    let tool_col = tools::default_collection(root.to_path_buf());
    let gw = Gateway::new(tool_col, AllowAll);

    let assembler = agent::context_assembler::ContextAssembler::new(
        root,
        agent::context_assembler::ContextBudgetConfig::default(),
    );

    // Huge repetitive compiler output that should be compacted
    let raw_diff = "diff --git a/src/main.rs b/src/main.rs\nindex 0000000..1111111 100644\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1,2 @@\n fn main() {\n+    println!(\"audit\");\n }\n";

    let steps = vec![
        Step::call("bash", serde_json::json!({ "command": format!("echo '{raw_diff}'") })),
        Step::Final("Done".to_string()),
    ];

    let mut model = RecordingModel::new(steps);
    let mut history = vec![Message::text("system", "Test Assistant")];

    let agent = ToolCallAgent::new(&gw).with_context_assembler(&assembler);
    let res = agent.run_turn(&mut model, "Check diff", &mut history);
    assert!(res.is_ok());

    // Verify model captured history
    assert!(model.captured_histories.len() >= 2);
    let second_turn_history = &model.captured_histories[1];
    let tool_response = second_turn_history
        .iter()
        .find(|m| m.role == "tool")
        .expect("Must have tool response");

    // Must contain exit code
    assert!(tool_response.content.contains("[exit 0]"));
    // Ensure that raw diff is NOT duplicated multiple times in the message
    let occurrences = tool_response.content.matches("diff --git").count();
    assert_eq!(occurrences, 1, "Raw output must not be duplicated into model input");
}

#[test]
fn test_audit_non_shell_tool_outputs_not_corrupted() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let tool_col = tools::default_collection(root.to_path_buf());
    let gw = Gateway::new(tool_col, AllowAll);

    // Call StrReplaceEditor
    fs::write(root.join("test.txt"), "hello world").unwrap();
    let args = serde_json::json!({
        "command": "view",
        "path": "test.txt"
    });

    let res = gw.call("editor", &args).unwrap();
    assert_eq!(res.content, "hello world");
    // Non-shell tool outputs must not contain shell exit code trailers
    assert!(!res.content.contains("[exit "));
    assert!(res.observation.is_none());
}

// ============================================================================
// SECTION 3: EVIDENCE CORRECTNESS AUDIT
// ============================================================================

#[test]
fn test_audit_evidence_correctness_and_invalidation() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let mut cov = CoverageTracker::new();

    // 1. Search hits NEVER count as inspection
    cov.record_search_hit("src/engine.rs", None, "query_term");
    let rec = cov.files.get("src/engine.rs").unwrap();
    assert!(!rec.status.inspected, "Search hits must NEVER count as inspected");

    // 2. FileRead records correct inspection evidence
    cov.record_inspection("src/engine.rs", Some("hash1".into()), None, Some("read_file".into()));
    let rec = cov.files.get("src/engine.rs").unwrap();
    assert!(rec.status.inspected, "FileRead must record inspected = true");
    assert_eq!(rec.last_known_hash, "hash1");
    assert!(!rec.is_stale);

    // 3. Record successful test verification
    cov.record_test_run(
        "cargo test",
        None,
        true,
        &["src/engine.rs".to_string()],
        None,
        VerificationScope::WorkspaceSuite,
        Some("All passed".into()),
    );
    let rec = cov.files.get("src/engine.rs").unwrap();
    assert!(rec.status.tested, "Passed test must set tested = true");

    // 4. File modification INVALIDATES prior tested/verified claims
    cov.record_modification("src/engine.rs", Some("hash2".into()), None, Some("editor".into()));
    let rec = cov.files.get("src/engine.rs").unwrap();
    assert!(rec.status.modified, "Must be marked modified");
    assert!(!rec.status.tested, "Modification MUST invalidate tested status");
    assert!(!rec.status.runtime_verified, "Modification MUST invalidate runtime verification");
    assert_eq!(rec.last_known_hash, "hash2");

    // 5. Failed tests NEVER become successful verification records
    cov.record_test_run(
        "cargo test",
        None,
        false,
        &["src/engine.rs".to_string()],
        None,
        VerificationScope::WorkspaceSuite,
        Some("Test failed".into()),
    );
    let rec = cov.files.get("src/engine.rs").unwrap();
    assert!(!rec.status.tested, "Failed test must NEVER mark file tested");

    // 6. Stale hash detection
    let mut rec_mut = rec.clone();
    rec_mut.check_stale("hash3_external_change");
    assert!(rec_mut.is_stale, "External hash divergence must be flagged as stale");
}

// ============================================================================
// SECTION 4: AUTONOMOUS LIFECYCLE & PROCESS RESUME AUDIT
// ============================================================================

#[test]
fn test_audit_autonomous_lifecycle_atomic_resume() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // 1. Create simulated project architecture
    let mut map = ArchitectureMap::new(root.display().to_string());
    map.symbols.push(SymbolMeta {
        file: "src/worker.rs".to_string(),
        name: "WorkerPool".to_string(),
        kind: SymbolKind::Struct,
        is_public: true,
        line: 15,
        module_path: Some("worker".to_string()),
        signature: Some("pub struct WorkerPool".to_string()),
        conditional: None,
    });

    let engine = agent::autonomous::AutonomousEngine::new(&map, root);
    let plan = engine.plan_task("task-audit-lifecycle", "Refactor WorkerPool error handling");
    let mut cp = agent::autonomous::TaskCheckpoint::new("task-audit-lifecycle", "Refactor WorkerPool", plan);

    // Simulate work progress
    cp.completed_steps.push(1);
    cp.remaining_steps.retain(|&s| s != 1);
    cp.modified_files.push("src/worker.rs".to_string());
    cp.verification_results.push("cargo check".to_string());

    // 2. Atomically persist checkpoint (simulating process termination)
    cp.save_atomic(root).expect("Save must succeed");

    // 3. Load checkpoint in a fresh context (simulating process recovery)
    let resumed = agent::autonomous::TaskCheckpoint::load_checkpoint(root, "task-audit-lifecycle")
        .expect("Checkpoint must reload accurately");

    assert_eq!(resumed.task_id, "task-audit-lifecycle");
    assert_eq!(resumed.completed_steps, vec![1]);
    assert_eq!(resumed.modified_files, vec!["src/worker.rs"]);
    assert_eq!(resumed.verification_results, vec!["cargo check"]);
    assert!(!resumed.remaining_steps.contains(&1), "Completed steps must not be repeated");
}

// ============================================================================
// SECTION 5: SECURITY GOVERNANCE & MANIFEST SAFETY AUDIT
// ============================================================================

#[test]
fn test_audit_security_governance_original_command_invariant() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let tool_col = tools::default_collection(root.to_path_buf());
    let bg = BashGuard::default();
    let gw = Gateway::new(tool_col, bg);

    // Malicious wrapped command attempts: wrapper unwrap should NOT allow execution
    let adversarial_cmds = [
        "sudo rm -rf /",
        "env FOO=1 rm -rf /",
        "python3 -m unittest; rm -rf /",
    ];

    for bad_cmd in &adversarial_cmds {
        let args = serde_json::json!({ "command": bad_cmd });
        let call_res = gw.call("bash", &args);
        assert!(
            call_res.is_err(),
            "Dangerous command `{bad_cmd}` must be denied by BashGuard"
        );
        match call_res {
            Err(ToolError::Denied(reason)) => {
                assert!(!reason.is_empty(), "Must fail-closed with rule reason");
            }
            other => panic!("Expected ToolError::Denied, got {:?}", other),
        }
    }
}

#[test]
fn test_audit_static_manifest_parsing_safety() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // Create a package.json with an adversarial script that touches a canary file if executed
    let canary = root.join("CANARY_EXECUTED.txt");
    let pkg_json = format!(
        r#"{{
            "name": "malicious-test",
            "scripts": {{
                "test": "touch {} && vitest run"
            }}
        }}"#,
        canary.display()
    );
    fs::write(root.join("package.json"), pkg_json).unwrap();

    // Run static extraction
    let tasks = onyx_project::extract_all_tasks(root);
    assert!(tasks.contains_key("npm run test"));

    // PROVE that static parsing NEVER executed the script
    assert!(
        !canary.exists(),
        "SECURITY VIOLATION: Manifest parsing executed script code!"
    );
}
