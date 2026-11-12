use std::fs;
use tempfile::tempdir;

use agent::context_assembler::{ContextAssembler, ContextBudgetConfig};
use agent::model::HttpModel;
use agent::{AgentEvent, Message, Model, Step, ToolCallAgent};
use onyx_project::{
    ArchitectureMap, CoverageTracker, DependencyEdge, EdgeKind, FileMeta, ProjectPitfall,
    ProjectScanner, SymbolKind, SymbolMeta, TaskSummary, WorkMemory,
};
use tools::audit::Gateway;
use tools::policy::AllowAll;
use tools::ToolCollection;

fn setup_test_workspace() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // 1. Create .onyx/project directory
    let proj_dir = root.join(".onyx/project");
    fs::create_dir_all(&proj_dir).unwrap();

    // 2. Create ArchitectureMap
    let mut arch = ArchitectureMap::new(root.to_string_lossy().to_string());
    arch.files.insert(
        "crates/agent/src/lib.rs".to_string(),
        FileMeta {
            path: "crates/agent/src/lib.rs".to_string(),
            language: "rust".to_string(),
            sha256: "hash_agent_lib".to_string(),
            size_bytes: 1024,
            last_scanned_at: 1000,
        },
    );
    arch.files.insert(
        "crates/tools/src/lib.rs".to_string(),
        FileMeta {
            path: "crates/tools/src/lib.rs".to_string(),
            language: "rust".to_string(),
            sha256: "hash_tools_lib".to_string(),
            size_bytes: 2048,
            last_scanned_at: 1000,
        },
    );

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
    arch.symbols.push(SymbolMeta {
        file: "crates/tools/src/lib.rs".to_string(),
        name: "ToolCollection".to_string(),
        kind: SymbolKind::Struct,
        is_public: true,
        line: 45,
        module_path: Some("tools".to_string()),
        signature: Some("pub struct ToolCollection".to_string()),
        conditional: None,
    });

    arch.dependencies.push(DependencyEdge {
        from_file: "crates/agent/src/lib.rs".to_string(),
        to_target: "crates/tools/src/lib.rs".to_string(),
        kind: EdgeKind::Import,
        is_dynamic: false,
        confidence: 1.0,
    });

    // Save architecture.json
    let arch_json = serde_json::to_string_pretty(&arch).unwrap();
    fs::write(proj_dir.join("architecture.json"), arch_json).unwrap();

    // 3. Create CoverageTracker
    let mut cov = CoverageTracker::new();
    cov.record_inspection(
        "crates/agent/src/lib.rs",
        Some("hash_agent_lib".to_string()),
        Some("sess_1".to_string()),
        Some("Inspected agent loop".to_string()),
    );
    // Mark tools as stale by simulating disk hash drift
    cov.record_inspection(
        "crates/tools/src/lib.rs",
        Some("hash_old_tools".to_string()),
        Some("sess_0".to_string()),
        None,
    );
    cov.check_stale_against_hashes(&{
        let mut map = std::collections::HashMap::new();
        map.insert(
            "crates/tools/src/lib.rs".to_string(),
            "hash_tools_lib".to_string(),
        );
        map
    });
    cov.save_atomic(root).unwrap();

    // 4. Create WorkMemory
    let mut mem = WorkMemory::new();
    let mut task = TaskSummary::new("task-42", "Refactor ToolCallAgent to use context assembler");
    task.components_involved = vec!["crates/agent".to_string()];
    task.files_modified = vec!["crates/agent/src/lib.rs".to_string()];
    task.decisions = vec!["Keep project intelligence outside system instruction budget".to_string()];
    mem.record_task_summary(task);

    mem.record_pitfall(
        "Context Budget Overflow",
        "Never dump full architecture.json or raw file trees directly into LLM prompts.",
        vec!["crates/agent".to_string()],
    );
    mem.save_atomic(root).unwrap();

    dir
}

#[test]
fn test_context_assembler_relevance_and_budget() {
    let dir = setup_test_workspace();
    let root = dir.path();

    let cfg = ContextBudgetConfig {
        max_tokens: 500,
        max_components: 4,
        max_symbols: 8,
        max_tasks: 2,
        max_pitfalls: 2,
    };
    let assembler = ContextAssembler::new(root, cfg);

    // Query relevant to agent and tools
    let (ctx, stats) = assembler.assemble_context("Refactor ToolCallAgent in agent crate");

    assert!(!ctx.is_empty(), "Context must not be empty for relevant query");
    assert!(ctx.contains("ToolCallAgent"), "Context must contain matching symbol");
    assert!(ctx.contains("crates/agent/src/lib.rs"), "Context must contain matching component");
    assert!(ctx.contains("Investigation Coverage:"), "Context must include coverage section");
    assert!(ctx.contains("Past Work & Decisions:"), "Context must include relevant work memory");
    assert!(ctx.contains("Known Architectural Pitfalls:"), "Context must include relevant pitfalls");

    // Telemetry assertions
    assert!(stats.components_matched >= 1);
    assert!(stats.symbols_included >= 1);
    assert!(stats.tasks_included >= 1);
    assert!(stats.pitfalls_included >= 1);
    assert!(stats.estimated_tokens <= 500, "Must respect max_tokens budget");
}

#[test]
fn test_context_assembler_stale_detection_in_prompt() {
    let dir = setup_test_workspace();
    let root = dir.path();

    let cfg = ContextBudgetConfig::default();
    let assembler = ContextAssembler::new(root, cfg);

    // Query matching tools
    let (ctx, stats) = assembler.assemble_context("Inspect ToolCollection in tools crate");

    assert!(ctx.contains("crates/tools/src/lib.rs"));
    assert!(ctx.contains("⚠️ STALE"), "Prompt context must explicitly alert model that tools file is stale");
    assert_eq!(stats.stale_count, 1);
}

#[test]
fn test_graceful_fallback_on_unrelated_query() {
    let dir = setup_test_workspace();
    let root = dir.path();

    let cfg = ContextBudgetConfig::default();
    let assembler = ContextAssembler::new(root, cfg);

    // Query with no substantive project keywords
    let (ctx, stats) = assembler.assemble_context("hello there");
    assert!(ctx.is_empty(), "Unrelated queries should not pollute prompt with project context");
    assert_eq!(stats.components_matched, 0);
}

#[test]
fn test_budget_enforcement_and_truncation() {
    let dir = setup_test_workspace();
    let root = dir.path();

    // Very small token budget to trigger truncation
    let cfg = ContextBudgetConfig {
        max_tokens: 30, // ~120 characters
        max_components: 4,
        max_symbols: 8,
        max_tasks: 2,
        max_pitfalls: 2,
    };
    let assembler = ContextAssembler::new(root, cfg);

    let (ctx, _stats) = assembler.assemble_context("Refactor ToolCallAgent and ToolCollection");
    assert!(ctx.contains("... [context truncated to stay within token budget]"));
}

#[test]
fn test_agent_integration_with_http_model_system_prompt() {
    let dir = setup_test_workspace();
    let root = dir.path();

    let cfg = ContextBudgetConfig::default();
    let assembler = ContextAssembler::new(root, cfg);

    let tools = ToolCollection::new();
    let policy = AllowAll;
    let gw = Gateway::new(tools, policy);

    let agent = ToolCallAgent::new(&gw).with_context_assembler(&assembler);

    let mut model = HttpModel::new(
        "127.0.0.1".into(),
        8080,
        "test-model".into(),
        "test-key".into(),
        "tool catalog".into(),
    );

    let mut history = Vec::new();
    let mut events = Vec::new();

    // Simulate turn run
    let user_msg = "Please update ToolCallAgent logic";
    let _ = agent.run_turn_observed(&mut model, user_msg, &mut history, &mut |ev| {
        events.push(ev);
    });

    // Verify turn tools received project context
    assert!(model.turn_tools.is_some());
    let turn = model.turn_tools.unwrap();
    assert!(turn.project_context.contains("ToolCallAgent"));
    assert!(turn.project_context.contains("crates/agent/src/lib.rs"));

    // Verify event emitted
    let has_grounding_event = events.iter().any(|ev| {
        if let AgentEvent::CapabilitySelection(msg) = ev {
            msg.contains("project intelligence: grounded")
        } else {
            false
        }
    });
    assert!(has_grounding_event, "Must emit project intelligence grounding event for observability");
}
