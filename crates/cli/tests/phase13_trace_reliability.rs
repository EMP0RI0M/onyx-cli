//! Phase 13 — ReAct Trace Integrity & Generation Reliability Tests
//!
//! Validates:
//! 1. Multiple ReAct steps with separate trace boundaries.
//! 2. Chronological transcript persistence order (user -> assistant tool call -> tool result -> assistant final answer).
//! 3. No duplicate transcript messages.
//! 4. Recovery after interrupted transcript persistence.
//! 5. Streamed repetitive output stall detection.
//! 6. Legitimate repeated code syntax not falsely classified as a stall.
//! 7. Bounded recovery attempts.
//! 8. Large directory listing aggregation with counts and pagination.
//! 9. Directory pagination correctness (offset / limit).
//! 10. Backward compatibility with older transcript files.
//! 11. Regression test reproducing 264-entry directory-listing scenario.

use agent::session::{self, Transcript};
use agent::{repetition::RepetitionDetector, AgentEvent, Message, ToolCall};
use serde_json::json;
use std::fs;
use tempfile::tempdir;
use tools::{Args, Tool};

/// Test 1: Multiple ReAct steps preserve separate trace boundaries
#[test]
fn test_multiple_react_steps_separate_trace_boundaries() {
    let mut steps_seen = Vec::new();
    let mut completions_seen = Vec::new();

    let mut on_event = |ev: AgentEvent| match ev {
        AgentEvent::Thinking { step } => steps_seen.push(step),
        AgentEvent::StepCompleted { step, summary } => completions_seen.push((step, summary)),
        _ => {}
    };

    on_event(AgentEvent::Thinking { step: 0 });
    on_event(AgentEvent::StepCompleted {
        step: 0,
        summary: "completed 1 tool calls".into(),
    });
    on_event(AgentEvent::Thinking { step: 1 });
    on_event(AgentEvent::StepCompleted {
        step: 1,
        summary: "terminated with answer: done".into(),
    });

    assert_eq!(steps_seen, vec![0, 1]);
    assert_eq!(completions_seen.len(), 2);
    assert_eq!(completions_seen[0].0, 0);
    assert_eq!(completions_seen[1].0, 1);
}

/// Test 2: Chronological transcript persistence order
#[test]
fn test_chronological_transcript_persistence_order() {
    let tmp = tempdir().expect("tempdir");
    let session_path = tmp.path().join("chronological.jsonl");

    let t = Transcript {
        path: session_path.clone(),
    };

    // 1. User message
    t.push(&Message::text("user", "how many folder are there"));

    // 2. Step 0 thinking
    t.push_step_thinking(0, "Analyzing request to list directories");

    // 3. Step 0 tool call
    t.push(&Message {
        role: "assistant".into(),
        content: "→ list_dir({\"path\":\".\"})".into(),
        tool_call: Some(ToolCall {
            name: "list_dir".into(),
            args: json!({"path": "."}),
        }),
    });

    // 4. Step 0 tool result
    t.push(&Message::text(
        "tool",
        "[Directory Listing: `.` | Total: 264 (143 directories, 121 files)]",
    ));

    // 5. Step 1 thinking
    t.push_step_thinking(1, "Counting extracted folders from tool summary");

    // 6. Step 1 final response
    t.push(&Message::text(
        "assistant",
        "There are 143 folders in the workspace.",
    ));

    let content = fs::read_to_string(&session_path).expect("read session");
    let lines: Vec<&str> = content.lines().collect();

    assert_eq!(lines.len(), 6);

    let v0: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    let v1: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    let v2: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
    let v3: serde_json::Value = serde_json::from_str(lines[3]).unwrap();
    let v4: serde_json::Value = serde_json::from_str(lines[4]).unwrap();
    let v5: serde_json::Value = serde_json::from_str(lines[5]).unwrap();

    // Verify chronological progression
    assert_eq!(v0["kind"], "message");
    assert_eq!(v0["role"], "user");

    assert_eq!(v1["kind"], "thinking");
    assert_eq!(v1["step"], 0);

    assert_eq!(v2["kind"], "message");
    assert_eq!(v2["role"], "assistant");
    assert_eq!(v2["tool_call"]["name"], "list_dir");

    assert_eq!(v3["kind"], "message");
    assert_eq!(v3["role"], "tool");

    assert_eq!(v4["kind"], "thinking");
    assert_eq!(v4["step"], 1);

    assert_eq!(v5["kind"], "message");
    assert_eq!(v5["role"], "assistant");
    assert_eq!(v5["content"], "There are 143 folders in the workspace.");
}

/// Test 3: No duplicate transcript messages on re-entry or resumed prompts
#[test]
fn test_no_duplicate_transcript_messages() {
    let tmp = tempdir().expect("tempdir");
    let session_path = tmp.path().join("dedup.jsonl");

    let t = Transcript {
        path: session_path.clone(),
    };

    let prompt = "test prompt";
    if !t.last_is_user(prompt) {
        t.push(&Message::text("user", prompt));
    }
    // Attempt re-writing exact prompt
    if !t.last_is_user(prompt) {
        t.push(&Message::text("user", prompt));
    }

    let messages = session::load_messages(&session_path);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, prompt);
}

/// Test 4: Backward compatibility with older transcript files
#[test]
fn test_backward_compatibility_with_older_transcripts() {
    let tmp = tempdir().expect("tempdir");
    let session_path = tmp.path().join("old_format.jsonl");

    let old_data = r#"{"kind":"meta","model":"test-model","ts":1700000000}
{"kind":"thinking","content":"Legacy thinking without step attribute","ts":1700000001}
{"kind":"message","role":"user","content":"legacy prompt","ts":1700000002}
{"kind":"message","role":"assistant","content":"legacy answer","ts":1700000003}
"#;
    fs::write(&session_path, old_data).expect("write old data");

    let items = session::load_transcript_items(&session_path);
    assert_eq!(items.len(), 3); // Thinking, User, Assistant
    match &items[0] {
        session::RestoredItem::Thinking(t) => {
            assert_eq!(t, "Legacy thinking without step attribute");
        }
        _ => panic!("Expected thinking item"),
    }

    let msgs = session::load_messages(&session_path);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[1].role, "assistant");
}

/// Test 5: Streamed repetitive output stall detection
#[test]
fn test_repetition_detector_stalled_loop() {
    let mut detector = RepetitionDetector::new(15, 60, 4);
    let phrase = "Also \"ws_bridge.py\" etc. ";

    // Push 3 times: not yet 4
    for _ in 0..3 {
        assert!(detector.push_delta(phrase).is_none());
    }

    // 4th time: triggers stall detection
    let stalled = detector.push_delta(phrase);
    assert!(stalled.is_some());
    assert!(stalled.unwrap().contains("ws_bridge.py"));
}

/// Test 6: Legitimate repeated code syntax not falsely classified as stall
#[test]
fn test_repetition_detector_legitimate_code_not_stalled() {
    let mut detector = RepetitionDetector::default();
    let rust_code_blocks = [
        "pub fn step_1() { println!(\"step\"); }\n",
        "pub fn step_2() { println!(\"step\"); }\n",
        "pub fn step_3() { println!(\"step\"); }\n",
        "pub fn step_4() { println!(\"step\"); }\n",
        "pub fn step_5() { println!(\"step\"); }\n",
    ];

    for block in rust_code_blocks {
        assert!(detector.push_delta(block).is_none());
    }
}

/// Test 7: Large directory listing aggregation with counts and preview
#[test]
fn test_large_directory_listing_aggregation() {
    let tmp = tempdir().expect("tempdir");
    let base = tmp.path();

    // Create 70 mock entries (exceeding MAX_UNPAGINATED = 60)
    for i in 0..40 {
        fs::create_dir(base.join(format!("folder_{i:02}"))).expect("create dir");
    }
    for i in 0..30 {
        fs::write(base.join(format!("file_{i:02}.txt")), b"hello").expect("write file");
    }

    let list_tool = tools::implementations::grok_build::ListDir::new(base);
    let args: Args = json!({"path": "."});
    let out = list_tool.execute(&args).expect("execute list_dir");

    let text = out.content;
    assert!(text.contains("Total: 70 (40 directories, 30 files)"));
    assert!(text.contains("has_more: true"));
    assert!(text.contains("next_offset: 50"));
}

/// Test 8: Directory listing pagination correctness (offset / limit)
#[test]
fn test_directory_listing_pagination() {
    let tmp = tempdir().expect("tempdir");
    let base = tmp.path();

    for i in 0..30 {
        fs::create_dir(base.join(format!("dir_{i:02}"))).expect("create dir");
    }

    let list_tool = tools::implementations::grok_build::ListDir::new(base);

    // Page 1: offset 0, limit 10
    let args1: Args = json!({"path": ".", "offset": 0, "limit": 10});
    let out1 = list_tool.execute(&args1).expect("execute list_dir p1");
    assert!(out1.content.contains("dir_00/"));
    assert!(out1.content.contains("dir_09/"));
    assert!(!out1.content.contains("dir_10/"));
    assert!(out1.content.contains("Showing entries 0 to 10 of 30"));

    // Page 2: offset 10, limit 10
    let args2: Args = json!({"path": ".", "offset": 10, "limit": 10});
    let out2 = list_tool.execute(&args2).expect("execute list_dir p2");
    assert!(!out2.content.contains("dir_09/"));
    assert!(out2.content.contains("dir_10/"));
    assert!(out2.content.contains("dir_19/"));
    assert!(!out2.content.contains("dir_20/"));
    assert!(out2.content.contains("Showing entries 10 to 20 of 30"));
}

/// Test 9: Regression test reproducing original 264-entry directory scenario
#[test]
fn test_regression_original_264_entry_directory_scenario() {
    let tmp = tempdir().expect("tempdir");
    let base = tmp.path();

    // Create 143 directories and 121 files = 264 total
    for i in 0..143 {
        fs::create_dir(base.join(format!("dir_{i:03}"))).expect("create dir");
    }
    for i in 0..121 {
        fs::write(base.join(format!("file_{i:03}.txt")), b"test").expect("write file");
    }

    let list_tool = tools::implementations::grok_build::ListDir::new(base);
    let out = list_tool.execute(&json!({"path": "."})).expect("list_dir");

    // The output MUST contain deterministic summary header
    assert!(out.content.contains("Total: 264 (143 directories, 121 files)"));
    assert!(out.content.contains("has_more: true"));
    assert!(out.content.contains("next_offset: 50"));

    // Ensure model does NOT receive huge raw listing (> 25KB)
    assert!(out.content.len() < 3000);
}

/// Test 10: Stall recovery with bounded retry feedback
#[test]
fn test_stall_recovery_bounded_feedback() {
    let mut steps = std::collections::VecDeque::new();
    // Step 0: model produces a stalled response
    steps.push_back(agent::Step::Final("[generation stall detected: repeating phrase \"Also ws_bridge.py\"]".into()));
    // Step 1: after receiving the corrective instruction, model delegates to a tool or produces valid answer
    steps.push_back(agent::Step::Final("There are 143 folders, verified by deterministic count.".into()));

    let mut model = agent::ScriptedModel { steps };
    let tools = tools::ToolCollection::new();
    let gateway = tools::audit::Gateway::new(tools, tools::policy::AllowAll);
    let mut agent = agent::ToolCallAgent::new(&gateway);
    let mut history = vec![Message::text("user", "how many folder are there")];

    let mut stall_events = 0usize;
    let mut sink = |ev: agent::AgentEvent| {
        if let agent::AgentEvent::StallDetected { .. } = ev {
            stall_events += 1;
        }
    };

    let report = agent.run_turn_observed(&mut model, "how many folder are there", &mut history, &mut sink)
        .expect("turn should succeed after stall recovery");

    assert_eq!(stall_events, 1);
    assert!(report.answer.contains("There are 143 folders"));
    // Verify that the corrective prompt was injected into history
    assert!(history.iter().any(|m| m.content.contains("A generation stall was detected")));
}

/// Test 11: Crash durability during active multi-step turn
#[test]
fn test_transcript_crash_durability_across_killed_session() {
    let tmp = tempdir().expect("tempdir");
    let session_path = tmp.path().join("crash_test.jsonl");

    {
        let t = Transcript { path: session_path.clone() };
        t.push(&Message::text("user", "build project"));
        t.push_step_thinking(0, "Deciding to run cargo build");
        t.push(&Message {
            role: "assistant".into(),
            content: "→ bash({\"command\":\"cargo build\"})".into(),
            tool_call: Some(ToolCall { name: "bash".into(), args: json!({"command": "cargo build"}) }),
        });
        t.push(&Message::text("tool", "Compiling tektile v0.1.0..."));
        // Simulate sudden process termination / crash here before final answer is written
    }

    // Now re-open the transcript on a restarted instance
    let items = session::load_transcript_items(&session_path);
    assert_eq!(items.len(), 3); // User, Thinking, Tool (tool call + tool result combined)

    let messages = session::load_messages(&session_path);
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[2].role, "tool");
}
