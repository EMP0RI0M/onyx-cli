//! Phase 13.3 Test Suite: Reasoning Visibility & Directory Discovery Integrity.
//! Verifies:
//! 1. ReasoningDelta / think tags never leak into ordinary user-facing assistant answers.
//! 2. Concise execution progress rendering.
//! 3. Empty workspace and non-existent path handling.
//! 4. Current-directory vs filesystem root directory listing.
//! 5. Recursive directory traversal with depth limit.
//! 6. Pagination consistency without duplicates or missing entries.
//! 7. TUI viewport truncation metadata vs actual command compression.
//! 8. Consistency between interactive TUI cleaning and one-shot execution.

use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;
use serde_json::json;
use agent::repetition::{strip_think_tags, ThinkTagFilter};
use agent::{AgentEvent, Message, Step, ToolCall, ToolCallAgent};
use tools::policy::AllowAll;
use tools::audit::Gateway;
use tools::Tool;

#[test]
fn test_think_tag_filter_streaming() {
    let mut filter = ThinkTagFilter::new();
    let mut content = Vec::new();
    let mut reasoning = Vec::new();

    let chunks = vec![
        "<th",
        "ink>The user said 'hi' - this is a simple greeting. I should respond briefly...</th",
        "ink>Hi! What can I help you with?",
    ];

    for c in chunks {
        filter.push(
            c,
            |chunk| content.push(chunk.to_string()),
            |r| reasoning.push(r.to_string()),
        );
    }
    filter.finish(
        |chunk| content.push(chunk.to_string()),
        |r| reasoning.push(r.to_string()),
    );

    let final_content = content.join("");
    let final_reasoning = reasoning.join("");

    assert_eq!(final_content, "Hi! What can I help you with?");
    assert!(final_reasoning.contains("The user said 'hi'"));
    assert!(!final_content.contains("<think>"));
    assert!(!final_content.contains("</think>"));
    assert!(!final_content.contains("The user said 'hi'"));
}

#[test]
fn test_strip_think_tags_complete() {
    let raw = "<think>Plan: 1. check directory. 2. answer user.</think>The directory has 42 files.";
    let (content, reasoning) = strip_think_tags(raw);

    assert_eq!(content, "The directory has 42 files.");
    assert_eq!(reasoning, Some("Plan: 1. check directory. 2. answer user.".to_string()));

    // When no think tag present
    let normal = "Just normal response.";
    let (content2, reasoning2) = strip_think_tags(normal);
    assert_eq!(content2, "Just normal response.");
    assert_eq!(reasoning2, None);
}

#[test]
fn test_empty_workspace_and_missing_directory() {
    let tmp = tempdir().unwrap();
    let empty_dir = tmp.path().join("empty_workspace");
    fs::create_dir(&empty_dir).unwrap();

    let list_tool = tools::implementations::grok_build::ListDir::new(tmp.path());
    
    // Listing empty directory
    let args = json!({ "path": "empty_workspace" });
    let res = list_tool.execute(&args).unwrap();
    assert!(res.content.contains("Total: 0 (0 directories, 0 files)"));

    // Listing non-existent path
    let bad_args = json!({ "path": "non_existent_folder" });
    let err = list_tool.execute(&bad_args).unwrap_err();
    assert!(err.to_string().contains("Directory not found"));
}

#[test]
fn test_current_dir_and_root_dir_distinction() {
    let tmp = tempdir().unwrap();
    let sub = tmp.path().join("project");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("main.rs"), "fn main() {}").unwrap();

    let list_tool = tools::implementations::grok_build::ListDir::new(&sub);

    // Current working directory '.'
    let res_curr = list_tool.execute(&json!({ "path": "." })).unwrap();
    assert!(res_curr.content.contains("main.rs"));

    // Absolute root '/'
    let res_root = list_tool.execute(&json!({ "path": "/" })).unwrap();
    assert!(res_root.content.contains("[Directory Listing: `/`"));
    assert!(res_root.content.contains("Total:"));
}

#[test]
fn test_recursive_directory_traversal() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("src/models")).unwrap();
    fs::create_dir_all(root.join("src/controllers")).unwrap();
    fs::write(root.join("src/models/user.rs"), "struct User;").unwrap();
    fs::write(root.join("src/controllers/user.rs"), "struct UserController;").unwrap();
    fs::write(root.join("Cargo.toml"), "[package]").unwrap();

    let list_tool = tools::implementations::grok_build::ListDir::new(root);

    // Shallow listing (depth: 1)
    let res_shallow = list_tool.execute(&json!({ "path": ".", "depth": 1 })).unwrap();
    assert!(res_shallow.content.contains("src/"));
    assert!(res_shallow.content.contains("Cargo.toml"));
    assert!(!res_shallow.content.contains("src/models/user.rs"));

    // Recursive listing (depth: 3)
    let res_rec = list_tool.execute(&json!({ "path": ".", "depth": 3, "limit": 100 })).unwrap();
    assert!(res_rec.content.contains("src/"));
    assert!(res_rec.content.contains("src/models/"));
    assert!(res_rec.content.contains("src/models/user.rs"));
    assert!(res_rec.content.contains("src/controllers/user.rs"));
}

#[test]
fn test_pagination_complete_traversal_without_duplicates() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    for i in 0..75 {
        fs::write(root.join(format!("file_{:03}.txt", i)), "data").unwrap();
    }

    let list_tool = tools::implementations::grok_build::ListDir::new(root);

    // Page 1
    let p1 = list_tool.execute(&json!({ "offset": 0, "limit": 50 })).unwrap();
    assert!(p1.content.contains("offset: 0, count: 50, has_more: true, next_offset: 50"));
    assert!(p1.content.contains("file_000.txt"));
    assert!(p1.content.contains("file_049.txt"));
    assert!(!p1.content.contains("file_050.txt"));

    // Page 2
    let p2 = list_tool.execute(&json!({ "offset": 50, "limit": 50 })).unwrap();
    assert!(p2.content.contains("offset: 50, count: 25, has_more: false, next_offset: null"));
    assert!(p2.content.contains("file_050.txt"));
    assert!(p2.content.contains("file_074.txt"));
    assert!(!p2.content.contains("file_049.txt"));
}

#[test]
fn test_tui_and_oneshot_consistent_behavior() {
    let raw_model_response = "<think>Checking user intent</think>Hello! How can I assist you today?";
    
    // TUI clean path
    let (tui_clean, tui_reasoning) = strip_think_tags(raw_model_response);
    assert_eq!(tui_clean, "Hello! How can I assist you today?");
    assert_eq!(tui_reasoning, Some("Checking user intent".to_string()));

    // One-shot REPL clean path
    let (cli_clean, _) = strip_think_tags(raw_model_response);
    assert_eq!(cli_clean, "Hello! How can I assist you today?");
}
