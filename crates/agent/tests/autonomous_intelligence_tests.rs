use std::fs;
use tempfile::tempdir;

use agent::autonomous::{AutonomousEngine, StepStatus, TaskCheckpoint};
use onyx_project::{ArchitectureMap, DependencyEdge, EdgeKind, FileMeta, SymbolKind, SymbolMeta, TaskSummary, WorkMemory};

fn build_test_arch() -> ArchitectureMap {
    let mut arch = ArchitectureMap::new("/root/tektile-core");

    arch.files.insert(
        "crates/agent/src/lib.rs".to_string(),
        FileMeta {
            path: "crates/agent/src/lib.rs".to_string(),
            language: "rust".to_string(),
            sha256: "hash_agent".to_string(),
            size_bytes: 5000,
            last_scanned_at: 100,
        },
    );
    arch.files.insert(
        "crates/tools/src/lib.rs".to_string(),
        FileMeta {
            path: "crates/tools/src/lib.rs".to_string(),
            language: "rust".to_string(),
            sha256: "hash_tools".to_string(),
            size_bytes: 3000,
            last_scanned_at: 100,
        },
    );
    arch.files.insert(
        "crates/agent/tests/autonomous_tests.rs".to_string(),
        FileMeta {
            path: "crates/agent/tests/autonomous_tests.rs".to_string(),
            language: "rust".to_string(),
            sha256: "hash_tests".to_string(),
            size_bytes: 2000,
            last_scanned_at: 100,
        },
    );

    // Dependencies
    arch.dependencies.push(DependencyEdge {
        from_file: "crates/agent/src/lib.rs".to_string(),
        to_target: "crates/tools/src/lib.rs".to_string(),
        kind: EdgeKind::Import,
        is_dynamic: false,
        confidence: 1.0,
    });
    arch.dependencies.push(DependencyEdge {
        from_file: "crates/agent/tests/autonomous_tests.rs".to_string(),
        to_target: "crates/agent/src/lib.rs".to_string(),
        kind: EdgeKind::Import,
        is_dynamic: false,
        confidence: 1.0,
    });

    // Symbols
    arch.symbols.push(SymbolMeta {
        file: "crates/agent/src/lib.rs".to_string(),
        name: "ToolCallAgent".to_string(),
        kind: SymbolKind::Struct,
        is_public: true,
        line: 135,
        module_path: Some("agent".to_string()),
        signature: Some("pub struct ToolCallAgent".to_string()),
        conditional: None,
    });

    arch
}

#[test]
fn test_architecture_aware_planning_and_impact() {
    let arch = build_test_arch();
    let dir = tempdir().unwrap();

    let mut mem = WorkMemory::new();
    let mut task = TaskSummary::new("past-1", "Refactor agent loop");
    task.files_modified = vec!["crates/agent/src/lib.rs".to_string()];
    task.regressions_discovered = vec!["Unbounded retries when tool returns denial".to_string()];
    mem.record_task_summary(task);

    let engine = AutonomousEngine::new(&arch, dir.path()).with_memory(&mem);

    // Plan task modifying ToolCallAgent
    let plan = engine.plan_task("task-auto-1", "Update ToolCallAgent execution loop");

    assert_eq!(plan.task_id, "task-auto-1");
    assert!(plan.affected_files.contains(&"crates/agent/src/lib.rs".to_string()));
    assert!(plan.steps.len() >= 4, "Must construct multi-stage execution plan");
    assert!(plan.potential_regressions.iter().any(|r| r.contains("Unbounded retries")));
    assert!(plan.targeted_tests.iter().any(|t| t.contains("autonomous_tests.rs")));
}

#[test]
fn test_automatic_test_selection() {
    let arch = build_test_arch();
    let dir = tempdir().unwrap();
    let engine = AutonomousEngine::new(&arch, dir.path());

    let changed = vec!["crates/agent/src/lib.rs".to_string()];
    let selected_tests = engine.select_tests_for_changes(&changed);

    assert!(!selected_tests.is_empty(), "Must identify affected tests");
    assert!(selected_tests.iter().any(|t| t.contains("autonomous_tests.rs")));
}

#[test]
fn test_compiler_error_diagnosis_reasoning() {
    let arch = build_test_arch();
    let dir = tempdir().unwrap();
    let engine = AutonomousEngine::new(&arch, dir.path());

    let compiler_output = r#"
error[E0432]: unresolved import `tools::audit::Gateway`
 --> crates/agent/src/lib.rs:18:5
  |
18 | use tools::audit::Gateway;
  |     ^^^^^^^^^^^^^^^^^^^^^ no `Gateway` in `audit`
"#;

    let diagnosis = engine.diagnose_failure("cargo check -p agent", compiler_output, 1);

    assert_eq!(diagnosis.error_file, Some("crates/agent/src/lib.rs".to_string()));
    assert_eq!(diagnosis.error_line, Some(18));
    assert!(diagnosis.error_message.contains("error[E0432]"));
    assert!(diagnosis.suggested_remedy.contains("Missing import or symbol in scope"));
}

#[test]
fn test_persistent_checkpoint_and_recovery() {
    let arch = build_test_arch();
    let dir = tempdir().unwrap();
    let root = dir.path();
    let engine = AutonomousEngine::new(&arch, root);

    let plan = engine.plan_task("task-recover-99", "Refactor agent runner");
    let mut checkpoint = TaskCheckpoint::new("task-recover-99", "Refactor agent runner", plan);

    checkpoint.completed_steps.push(1);
    checkpoint.remaining_steps.retain(|&id| id != 1);
    checkpoint.modified_files.push("crates/agent/src/lib.rs".to_string());

    checkpoint.save_atomic(root).expect("save checkpoint succeeds");

    // Verify recovery after simulated termination
    let restored = TaskCheckpoint::load_checkpoint(root, "task-recover-99").expect("restores checkpoint");

    assert_eq!(restored.task_id, "task-recover-99");
    assert_eq!(restored.completed_steps, vec![1]);
    assert!(!restored.remaining_steps.contains(&1));
    assert_eq!(restored.modified_files, vec!["crates/agent/src/lib.rs".to_string()]);
}
