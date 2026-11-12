//! Integration tests for hybrid capability discovery and dynamic schema expansion.

use std::sync::{Arc, Mutex};
use serde_json::{json, Value};
use agent::model::HttpModel;
use agent::retrieval::CapabilitySelector;
use agent::{AgentEvent, Message, Model, Step, ToolCall, ToolCallAgent};
use tools::policy::AllowAll;
use tools::{Args, Output, Tool, ToolCollection, ToolError};

struct MockTool {
    name: String,
    description: String,
}

impl Tool for MockTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn execute(&self, _args: &Args) -> Result<Output, ToolError> {
        Ok(Output::ok(format!("executed {}", self.name)))
    }
}

/// Scripted model to simulate model discovering a capability mid-run and then invoking it.
struct DiscoveryScriptedModel {
    steps: Mutex<Vec<Step>>,
}

impl Model for DiscoveryScriptedModel {
    fn as_any(&self) -> &(dyn std::any::Any + 'static) {
        self
    }
    fn as_any_mut(&mut self) -> &mut (dyn std::any::Any + 'static) {
        self
    }
    fn next_step(&mut self, _history: &[Message], _tools: &[&str]) -> Step {
        self.steps.lock().unwrap().remove(0)
    }
}

#[test]
fn test_hybrid_capability_discovery_mid_turn() {
    let mut tool_col = ToolCollection::new();
    tool_col.add(MockTool {
        name: "terminate".into(),
        description: "Terminate the task".into(),
    });
    tool_col.add(MockTool {
        name: "code_analyzer".into(),
        description: "Analyze AST and find bugs in Rust code".into(),
    });

    let gw = tools::audit::Gateway::new(tool_col, AllowAll);

    // Build Tool Registry with 15 tools so filtering activates (> 8)
    let mut tool_corpus = Vec::new();
    tool_corpus.push(onyx_retrieval::Tool {
        id: "terminate".into(),
        name: "terminate".into(),
        description: "Terminate the task".into(),
        experimental_searchable_description: None,
        input_schema: json!({}),
        output_schema: json!({}),
    });
    tool_corpus.push(onyx_retrieval::Tool {
        id: "code_analyzer".into(),
        name: "code_analyzer".into(),
        description: "Analyze AST and find bugs in Rust code".into(),
        experimental_searchable_description: None,
        input_schema: json!({"properties": {"file": {"type": "string"}}}),
        output_schema: json!({}),
    });
    for i in 1..=12 {
        let name = format!("dummy_tool_{i}");
        tool_corpus.push(onyx_retrieval::Tool {
            id: name.clone(),
            name: name.clone(),
            description: format!("Dummy tool {i} for general testing"),
            experimental_searchable_description: None,
            input_schema: json!({}),
            output_schema: json!({}),
        });
    }

    let sel = CapabilitySelector::new()
        .with_top_k(4)
        .with_min_catalog(8)
        .with_tools(tool_corpus);

    // Step 1: LLM calls search_capabilities("find bugs in Rust code")
    // Step 2: LLM calls newly mounted code_analyzer(file="main.rs")
    // Step 3: LLM terminates
    let model = DiscoveryScriptedModel {
        steps: Mutex::new(vec![
            Step::Calls(vec![ToolCall {
                name: "search_capabilities".into(),
                args: json!({"query": "find bugs in Rust code", "limit": 2}),
            }]),
            Step::Calls(vec![ToolCall {
                name: "code_analyzer".into(),
                args: json!({"file": "main.rs"}),
            }]),
            Step::Calls(vec![ToolCall {
                name: "terminate".into(),
                args: json!({"answer": "found and fixed"}),
            }]),
        ]),
    };

    let mut boxed_model: Box<dyn Model> = Box::new(model);
    let agent = ToolCallAgent::new(&gw).with_selector(&sel);

    let mut history = vec![Message::text("system", "You are onyx.")];
    let mut events = Vec::new();

    let res = agent.run_turn_observed(
        boxed_model.as_mut(),
        "Please inspect codebase",
        &mut history,
        &mut |ev| events.push(ev),
    );

    assert!(res.is_ok(), "turn execution failed: {:?}", res.err());
    let report = res.unwrap();
    assert_eq!(report.steps, 3);

    // Verify search_capabilities was executed and its result added to history
    let search_msg = history.iter().find(|m| m.role == "tool" && m.content.contains("code_analyzer"));
    assert!(search_msg.is_some(), "search_capabilities output missing from history");

    // Verify code_analyzer was invoked successfully
    let code_msg = history.iter().find(|m| m.role == "tool" && m.content.contains("executed code_analyzer"));
    assert!(code_msg.is_some(), "code_analyzer output missing from history");
}

#[test]
fn test_discovery_call_limits() {
    let mut tool_col = ToolCollection::new();
    tool_col.add(MockTool {
        name: "terminate".into(),
        description: "Terminate the task".into(),
    });
    let gw = tools::audit::Gateway::new(tool_col, AllowAll);
    let sel = CapabilitySelector::new();

    // 4 calls to search_capabilities (max limit is 3)
    let model = DiscoveryScriptedModel {
        steps: Mutex::new(vec![
            Step::Calls(vec![ToolCall {
                name: "search_capabilities".into(),
                args: json!({"query": "q1"}),
            }]),
            Step::Calls(vec![ToolCall {
                name: "search_capabilities".into(),
                args: json!({"query": "q2"}),
            }]),
            Step::Calls(vec![ToolCall {
                name: "search_capabilities".into(),
                args: json!({"query": "q3"}),
            }]),
            Step::Calls(vec![ToolCall {
                name: "search_capabilities".into(),
                args: json!({"query": "q4"}),
            }]),
            Step::Calls(vec![ToolCall {
                name: "terminate".into(),
                args: json!({}),
            }]),
        ]),
    };

    let mut boxed_model: Box<dyn Model> = Box::new(model);
    let agent = ToolCallAgent::new(&gw).with_selector(&sel);

    let mut history = vec![Message::text("system", "You are onyx.")];
    let res = agent.run_turn(boxed_model.as_mut(), "Test limits", &mut history);

    assert!(res.is_ok());
    let limit_hit = history.iter().any(|m| m.role == "tool" && m.content.contains("Discovery limit reached"));
    assert!(limit_hit, "Expected discovery limit message in history");
}
