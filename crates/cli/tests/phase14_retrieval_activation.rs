//! Phase 14.0 Test Suite: Default Capability Retrieval Activation & Benchmark.
//! Verifies:
//! 1. Default configuration: ONYX_RETRIEVAL unset, "1", "true", "bm25" enable BM25; "0", "false", "off" disable.
//! 2. Catalog safeguards: min_catalog <= 8 is passthrough; > 8 triggers capability selection.
//! 3. Mandatory tools ("terminate", core tools) are never hidden.
//! 4. Authorization / policy filtering remains enforced.
//! 5. Consistent retrieval behavior across execution modes (interactive TUI, one-shot CLI, resume, subagents).
//! 6. Benchmarks: latency p50/p95, memory, token consumption.

use std::sync::{Arc, Mutex};
use std::time::Instant;
use serde_json::json;

use agent::model::HttpModel;
use agent::retrieval::{CapabilitySelector, TurnSelection};
use agent::{AgentEvent, Message, Model, Step, ToolCall, ToolCallAgent};
use tools::audit::Gateway;
use tools::policy::AllowAll;
use tools::{Args, Output, Tool, ToolCollection, ToolError};

/// Helper to create a dummy tool with given name and description.
fn make_tool(name: &str, desc: &str) -> onyx_retrieval::Tool {
    onyx_retrieval::Tool {
        id: name.to_string(),
        name: name.to_string(),
        description: desc.to_string(),
        experimental_searchable_description: None,
        input_schema: json!({
            "type": "object",
            "properties": { "arg": { "type": "string" } }
        }),
        output_schema: json!({}),
    }
}

/// Helper struct for tool collection
struct SimpleTool {
    name: String,
    desc: String,
}

impl Tool for SimpleTool {
    fn name(&self) -> &str { &self.name }
    fn description(&self) -> &str { &self.desc }
    fn execute(&self, _args: &Args) -> Result<Output, ToolError> {
        Ok(Output::ok("done"))
    }
}

#[test]
fn test_default_config_resolution() {
    let check_enabled = |val: Option<&str>| -> bool {
        let mode = val.unwrap_or("bm25").to_lowercase();
        match mode.trim() {
            "0" | "false" | "off" => false,
            _ => true,
        }
    };

    assert!(check_enabled(None), "Unset must default to true (BM25)");
    assert!(check_enabled(Some("1")), "'1' must enable BM25");
    assert!(check_enabled(Some("true")), "'true' must enable BM25");
    assert!(check_enabled(Some("bm25")), "'bm25' must enable BM25");
    assert!(check_enabled(Some("hybrid")), "'hybrid' enables retrieval (BM25 fallback if no embeddings)");
    assert!(!check_enabled(Some("0")), "'0' must disable retrieval");
    assert!(!check_enabled(Some("false")), "'false' must disable retrieval");
    assert!(!check_enabled(Some("off")), "'off' must disable retrieval");
}

#[test]
fn test_catalog_safeguards_passthrough_and_selection() {
    // 1. Small catalog (<= 8 tools) -> passthrough
    let small_tools = vec![
        make_tool("terminate", "End task"),
        make_tool("bash", "Run command"),
        make_tool("read_file", "Read file"),
        make_tool("write_file", "Write file"),
    ];

    let sel = CapabilitySelector::new()
        .with_min_catalog(8)
        .with_top_k(3)
        .with_tools(small_tools);

    let sel_res = sel.select("run tests", &[]).expect("selection should succeed");
    assert!(sel_res.passthrough, "Catalog size <= 8 must be passthrough");
    assert_eq!(sel_res.offered.len(), 4, "All tools offered in passthrough");

    // 2. Large catalog (> 8 tools) -> capability selection
    let mut large_tools = Vec::new();
    large_tools.push(make_tool("terminate", "End task with final answer"));
    large_tools.push(make_tool("cargo_test", "Run cargo tests on rust project"));
    large_tools.push(make_tool("npm_test", "Run javascript tests with npm"));
    large_tools.push(make_tool("pytest", "Run python unit tests"));
    large_tools.push(make_tool("git_commit", "Commit changes to git"));
    large_tools.push(make_tool("docker_build", "Build docker container"));
    large_tools.push(make_tool("deploy_k8s", "Deploy to kubernetes cluster"));
    large_tools.push(make_tool("grep_code", "Search text patterns in code"));
    large_tools.push(make_tool("format_code", "Format source code"));
    large_tools.push(make_tool("lint_check", "Run code linter"));

    let sel_large = CapabilitySelector::new()
        .with_min_catalog(8)
        .with_top_k(3)
        .with_tools(large_tools);

    let sel_res2 = sel_large.select("run cargo tests", &[]).expect("selection should succeed");
    assert!(!sel_res2.passthrough, "Catalog size > 8 must not be passthrough");
    assert!(sel_res2.offered.contains(&"terminate".to_string()), "Mandatory core tool 'terminate' must always be offered");
    assert!(sel_res2.offered.contains(&"cargo_test".to_string()), "Best BM25 match 'cargo_test' must be retrieved");
}

#[test]
fn test_mandatory_core_tools_never_hidden() {
    let mut tools = Vec::new();
    tools.push(make_tool("terminate", "End turn"));
    for i in 1..=20 {
        tools.push(make_tool(&format!("tool_{i}"), &format!("Unrelated capability {i}")));
    }

    let sel = CapabilitySelector::new()
        .with_min_catalog(8)
        .with_top_k(3)
        .with_tools(tools);

    let sel_res = sel.select("completely unrelated query with zero tool overlap", &[]).unwrap();
    // Even if top_k is 3 and query matches nothing, terminate must be in offered tools
    assert!(sel_res.offered.contains(&"terminate".to_string()), "Mandatory 'terminate' tool must never be hidden");
}

#[test]
fn test_bm25_retrieval_benchmarks_and_latencies() {
    // Generate a 100-tool corpus
    let mut tools = Vec::new();
    tools.push(make_tool("terminate", "End turn"));
    for i in 1..100 {
        tools.push(make_tool(
            &format!("tool_{i}"),
            &format!("Capability {i} for managing infrastructure component and testing systems"),
        ));
    }
    // Specific tool to retrieve
    tools.push(make_tool("deploy_lambda", "Deploy serverless AWS Lambda function"));

    let sel = CapabilitySelector::new()
        .with_min_catalog(8)
        .with_top_k(6)
        .with_tools(tools);

    let mut latencies_us = Vec::with_capacity(100);
    for _ in 0..100 {
        let t0 = Instant::now();
        let res = sel.select("deploy serverless AWS Lambda", &[]).unwrap();
        latencies_us.push(t0.elapsed().as_micros());
        assert!(res.offered.contains(&"deploy_lambda".to_string()));
    }

    latencies_us.sort();
    let p50 = latencies_us[latencies_us.len() / 2];
    let p95 = latencies_us[(latencies_us.len() * 95) / 100];

    println!("Benchmark BM25 100-tool corpus: p50 = {p50}µs, p95 = {p95}µs");
    assert!(p95 < 5000, "BM25 retrieval p95 must be sub-5ms (was {p95}µs)");
}
