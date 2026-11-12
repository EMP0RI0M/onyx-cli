use std::fs;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

use agent::autonomous::{AutonomousEngine, StepStatus, TaskCheckpoint, is_engineering_task};
use agent::context_assembler::{ContextAssembler, ContextBudgetConfig};
use agent::{AgentEvent, Message, Model, Step, ToolCall, ToolCallAgent};
use onyx_project::{ArchitectureMap, DependencyEdge, EdgeKind, FileMeta, SymbolKind, SymbolMeta};
use tools::audit::Gateway;
use tools::policy::AllowAll;
use tools::ToolCollection;

fn build_test_arch(root: &str) -> ArchitectureMap {
    let mut arch = ArchitectureMap::new(root);

    arch.files.insert(
        "src/core.rs".to_string(),
        FileMeta {
            path: "src/core.rs".to_string(),
            language: "rust".to_string(),
            sha256: "hash_core".to_string(),
            size_bytes: 1200,
            last_scanned_at: 100,
        },
    );
    arch.files.insert(
        "src/lib.rs".to_string(),
        FileMeta {
            path: "src/lib.rs".to_string(),
            language: "rust".to_string(),
            sha256: "hash_lib".to_string(),
            size_bytes: 2500,
            last_scanned_at: 100,
        },
    );
    arch.files.insert(
        "tests/core_test.rs".to_string(),
        FileMeta {
            path: "tests/core_test.rs".to_string(),
            language: "rust".to_string(),
            sha256: "hash_test".to_string(),
            size_bytes: 800,
            last_scanned_at: 100,
        },
    );

    arch.dependencies.push(DependencyEdge {
        from_file: "src/lib.rs".to_string(),
        to_target: "src/core.rs".to_string(),
        kind: EdgeKind::Import,
        is_dynamic: false,
        confidence: 1.0,
    });
    arch.dependencies.push(DependencyEdge {
        from_file: "tests/core_test.rs".to_string(),
        to_target: "src/core.rs".to_string(),
        kind: EdgeKind::Import,
        is_dynamic: false,
        confidence: 1.0,
    });

    arch.symbols.push(SymbolMeta {
        file: "src/core.rs".to_string(),
        name: "ProcessEngine".to_string(),
        kind: SymbolKind::Struct,
        is_public: true,
        line: 10,
        module_path: Some("core".to_string()),
        signature: Some("pub struct ProcessEngine".to_string()),
        conditional: None,
    });

    arch
}

/// Scripted mock model simulating autonomous steps
struct ScriptedAutonomousModel {
    steps: std::collections::VecDeque<Step>,
}

impl ScriptedAutonomousModel {
    fn new(steps: Vec<Step>) -> Self {
        Self { steps: steps.into() }
    }
}

impl Model for ScriptedAutonomousModel {
    fn next_step(&mut self, _history: &[Message], _tools: &[&str]) -> Step {
        self.steps
            .pop_front()
            .unwrap_or_else(|| Step::Final("autonomous workflow completed".to_string()))
    }
    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}

#[test]
fn test_live_autonomous_lifecycle_integration() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // 1. Setup workspace folders
    let src_dir = root.join("src");
    let tests_dir = root.join("tests");
    fs::create_dir_all(&src_dir).unwrap();
    fs::create_dir_all(&tests_dir).unwrap();

    let core_file = src_dir.join("core.rs");
    fs::write(&core_file, "pub struct ProcessEngine;\n").unwrap();
    let lib_file = src_dir.join("lib.rs");
    fs::write(&lib_file, "mod core;\npub use core::ProcessEngine;\n").unwrap();
    let test_file = tests_dir.join("core_test.rs");
    fs::write(&test_file, "#[test]\nfn test_engine() {}\n").unwrap();

    // 2. Setup project intelligence and architecture
    let arch = build_test_arch(&root.to_string_lossy());
    let dot_onyx = root.join(".onyx/project");
    fs::create_dir_all(&dot_onyx).unwrap();
    fs::write(dot_onyx.join("architecture.json"), serde_json::to_string(&arch).unwrap()).unwrap();

    let user_request = "Refactor ProcessEngine and fix compiler warnings";
    assert!(is_engineering_task(user_request), "Query must be recognized as genuine engineering task");

    let engine = AutonomousEngine::new(&arch, root);
    let plan = engine.plan_task("task-auto-e2e", user_request);
    assert_eq!(plan.task_id, "task-auto-e2e");
    assert!(!plan.steps.is_empty());
    assert!(plan.targeted_tests.iter().any(|t| t.contains("core_test.rs")));

    let checkpoint = Arc::new(Mutex::new(TaskCheckpoint::new("task-auto-e2e", user_request, plan)));

    // 3. Setup Gateway with editor and bash
    let mut tool_col = ToolCollection::new();
    tool_col.add(tools::editor::StrReplaceEditor::new(root));
    tool_col.add(tools::bash::Bash::new());
    tool_col.add(tools::control::Terminate);

    let gw = Gateway::<dyn tools::policy::Policy>::from_boxed(tool_col, Box::new(AllowAll));

    let assembler = ContextAssembler::new(root, ContextBudgetConfig::default());

    // 4. Sequence of steps:
    //    Step 1: Edit src/core.rs
    //    Step 2: Run bash command that produces compiler error -> verifies Diagnostic Interceptor
    //    Step 3: Run bash command with fix -> verifies verification recording
    //    Step 4: Terminate
    let script = vec![
        Step::Calls(vec![ToolCall {
            name: "editor".to_string(),
            args: serde_json::json!({
                "command": "str_replace",
                "path": "src/core.rs",
                "old_str": "pub struct ProcessEngine;",
                "new_str": "pub struct ProcessEngine;\n// added field"
            }),
        }]),
        Step::Calls(vec![ToolCall {
            name: "bash".to_string(),
            args: serde_json::json!({
                "command": "sh -c 'echo \"error[E0432]: unresolved import `foo::bar`\" >&2; echo \"  --> src/core.rs:2:5\" >&2; exit 1'"
            }),
        }]),
        Step::Calls(vec![ToolCall {
            name: "bash".to_string(),
            args: serde_json::json!({
                "command": "cargo check --help"
            }),
        }]),
        Step::Calls(vec![ToolCall {
            name: "terminate".to_string(),
            args: serde_json::json!({
                "answer": "Engine refactoring and verification complete."
            }),
        }]),
    ];

    let mut model = ScriptedAutonomousModel::new(script);
    let mut agent = ToolCallAgent::new(&gw)
        .with_context_assembler(&assembler)
        .with_checkpoint(Arc::clone(&checkpoint));

    let mut history = Vec::new();
    let report = agent.run_turn(&mut model, user_request, &mut history).expect("agent turn completes");

    assert_eq!(report.answer, "Engine refactoring and verification complete.");

    // 5. Verify Diagnostic Interceptor in history
    let diagnostic_tool_msg = history.iter().find(|m| m.role == "tool" && m.content.contains("[Autonomous Diagnostic:"));
    assert!(diagnostic_tool_msg.is_some(), "Observation must contain structured diagnostic information");
    let diag_content = &diagnostic_tool_msg.unwrap().content;
    assert!(diag_content.contains("unresolved import"));
    assert!(diag_content.contains("src/core.rs"));
    assert!(diag_content.contains("Missing import or symbol in scope"));

    // 6. Verify Post-Edit Test Selection in history
    let test_plan_msg = history.iter().find(|m| m.role == "tool" && m.content.contains("[Post-Edit Verification Plan:"));
    assert!(test_plan_msg.is_some(), "Observation must contain post-edit verification plan for changed file");
    assert!(test_plan_msg.unwrap().content.contains("core_test.rs"));

    // 7. Verify Checkpoint was updated atomically
    let saved_cp = TaskCheckpoint::load_checkpoint(root, "task-auto-e2e").expect("Checkpoint file exists");
    assert!(saved_cp.modified_files.contains(&"src/core.rs".to_string()), "Modified file must be recorded");
    assert!(saved_cp.current_diagnostics.is_some(), "Diagnostic must be saved in checkpoint");
    assert!(saved_cp.verification_results.contains(&"cargo check --help".to_string()), "Verification must be recorded");

    // 8. Test Checkpoint Recovery and File Hash Validation
    let (unchanged, modified_or_missing) = saved_cp.validate_file_hashes(root);
    assert_eq!(unchanged, vec!["src/core.rs".to_string()], "Hash of src/core.rs matches recorded checkpoint hash");
    assert!(modified_or_missing.is_empty(), "No untracked drift");

    // Simulate external file tampering
    fs::write(&core_file, "// externally modified!").unwrap();
    let (_unchanged2, modified_or_missing2) = saved_cp.validate_file_hashes(root);
    assert_eq!(modified_or_missing2, vec!["src/core.rs".to_string()], "Detects file tampering after interruption");
}
