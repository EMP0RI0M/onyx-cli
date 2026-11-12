//! Phase 13.1 — Directory Pagination & Final-Response Stall Regression Tests
//!
//! Validates:
//! 1. Directory pagination schema and explicit metadata (total, offset, count, has_more, next_offset).
//! 2. String and integer argument normalization for offset and limit.
//! 3. Enforced maximum page size limit (caps to 100).
//! 4. Sequential multi-page directory traversal without duplicates or gaps.
//! 5. Exact interaction reproduction:
//!    - User asks: "how many folder are there" -> receives total & folder count.
//!    - User asks: "list all" -> retrieves pages sequentially using next_offset.
//! 6. Repetition stall protection on final-answer generation:
//!    - Detects word loops like "folders folders folders folders".
//!    - Safely halts and triggers bounded corrective recovery.

use agent::session::{self, Transcript};
use agent::{repetition::RepetitionDetector, AgentEvent, Message, Step, ToolCall};
use serde_json::json;
use std::fs;
use tempfile::tempdir;
use tools::{Args, Tool};

/// Test 1: Explicit pagination metadata and string argument normalization
#[test]
fn test_pagination_metadata_and_normalization() {
    let tmp = tempdir().expect("tempdir");
    let base = tmp.path();

    // Create 120 files
    for i in 0..120 {
        fs::write(base.join(format!("item_{i:03}.txt")), b"data").expect("write");
    }

    let list_tool = tools::implementations::grok_build::ListDir::new(base);

    // Call with string arguments (as models frequently emit)
    let args: Args = json!({
        "path": ".",
        "offset": "40",
        "limit": "30"
    });

    let out = list_tool.execute(&args).expect("execute list_dir");
    let content = out.content;

    assert!(content.contains("Total: 120 (0 directories, 120 files)"));
    assert!(content.contains("offset: 40"));
    assert!(content.contains("count: 30"));
    assert!(content.contains("has_more: true"));
    assert!(content.contains("next_offset: 70"));
    assert!(content.contains("item_040.txt"));
    assert!(content.contains("item_069.txt"));
    assert!(!content.contains("item_070.txt"));
}

/// Test 2: Maximum page size enforcement
#[test]
fn test_maximum_page_size_enforced() {
    let tmp = tempdir().expect("tempdir");
    let base = tmp.path();

    for i in 0..150 {
        fs::write(base.join(format!("item_{i:03}.txt")), b"data").expect("write");
    }

    let list_tool = tools::implementations::grok_build::ListDir::new(base);
    // Request limit 500 (exceeds MAX_PAGE_SIZE = 100)
    let args: Args = json!({
        "path": ".",
        "offset": 0,
        "limit": 500
    });

    let out = list_tool.execute(&args).expect("execute list_dir");
    let content = out.content;

    // Must be capped to 100
    assert!(content.contains("count: 100"));
    assert!(content.contains("next_offset: 100"));
    assert!(content.contains("has_more: true"));
}

/// Test 3: Sequential multi-page directory traversal without duplicates or missing items
#[test]
fn test_sequential_page_traversal_no_duplicates() {
    let tmp = tempdir().expect("tempdir");
    let base = tmp.path();

    // Create 85 items
    for i in 0..85 {
        fs::write(base.join(format!("file_{i:02}.log")), b"ok").expect("write");
    }

    let list_tool = tools::implementations::grok_build::ListDir::new(base);

    let mut collected = Vec::new();
    let mut offset = 0;
    let limit = 30;

    loop {
        let args: Args = json!({
            "path": ".",
            "offset": offset,
            "limit": limit
        });
        let out = list_tool.execute(&args).expect("execute");
        let content = out.content;

        for line in content.lines() {
            if line.starts_with("file_") {
                let name = line.split_whitespace().next().unwrap().to_string();
                collected.push(name);
            }
        }

        if content.contains("has_more: false") {
            break;
        }

        // Extract next_offset from header
        let next_str = content
            .split("next_offset: ")
            .nth(1)
            .and_then(|s| s.split(']').next())
            .expect("next_offset");
        offset = next_str.parse::<usize>().expect("parse next offset");
    }

    assert_eq!(collected.len(), 85);
    // Verify all 85 entries are present exactly once
    for i in 0..85 {
        let expected = format!("file_{i:02}.log");
        assert_eq!(collected.iter().filter(|&x| x == &expected).count(), 1);
    }
}

/// Test 4: Final-response word repetition stall detection
#[test]
fn test_final_response_word_repetition_detected() {
    let mut detector = RepetitionDetector::default();
    let stalled_text = "There are 179 folders folders folders folders folders folders (folders) in the current directory.";
    let detected = detector.push_delta(stalled_text);

    assert!(detected.is_some());
    let phrase = detected.unwrap();
    assert!(phrase.contains("folders"));
}

/// Test 5: ReAct loop recovery when final response stalls
#[test]
fn test_react_recovery_on_repetitive_final_response() {
    let mut steps = std::collections::VecDeque::new();
    // Step 0: Model produces repetitive final response
    steps.push_back(Step::Final("There are 179 folders folders folders folders folders folders in the current directory.".into()));
    // Step 1: Model receives stall warning and outputs clean response
    steps.push_back(Step::Final("There are 179 folders in the workspace.".into()));

    let mut model = agent::ScriptedModel { steps };
    let tools = tools::ToolCollection::new();
    let gateway = tools::audit::Gateway::new(tools, tools::policy::AllowAll);
    let agent = agent::ToolCallAgent::new(&gateway);
    let mut history = vec![Message::text("user", "how many folder are there")];

    let mut stall_emitted = false;
    let mut sink = |ev: AgentEvent| {
        if let AgentEvent::StallDetected { .. } = ev {
            stall_emitted = true;
        }
    };

    let report = agent.run_turn_observed(&mut model, "how many folder are there", &mut history, &mut sink)
        .expect("turn completes successfully after stall recovery");

    assert!(stall_emitted);
    assert_eq!(report.answer, "There are 179 folders in the workspace.");
    assert!(!report.answer.contains("folders folders"));
}
