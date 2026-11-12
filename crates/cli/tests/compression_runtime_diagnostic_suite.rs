use tools::compress::classify_with_meta;
use tools::bash::format_command_output;
use sandbox::SandboxedBash;

#[derive(Debug, serde::Serialize)]
pub struct DiagnosticTelemetry {
    pub command: String,
    pub original_bytes: usize,
    pub original_tokens_est: usize,
    pub compressed_bytes: usize,
    pub compressed_tokens_est: usize,
    pub selected_profile: Option<String>,
    pub applied: bool,
    pub bypass_reason: Option<String>,
    pub model_bound_bytes: usize,
    pub raw_output_duplicated: bool,
    pub tool_name: String,
}

pub fn instrument_command(tool_name: &str, cmd: &str, raw_output: &str, code: i32, is_success: bool) -> DiagnosticTelemetry {
    let orig_bytes = raw_output.len();
    let orig_tokens = (orig_bytes + 3) / 4;
    
    let meta = classify_with_meta(cmd);
    let selected_profile = meta.selected_profile.map(|p| format!("{:?}", p));
    
    // Execute format_command_output (canonical pipeline)
    let out = format_command_output(cmd, raw_output, code, is_success);
    let model_bound_bytes = out.content.len();
    let compressed_tokens = (model_bound_bytes + 3) / 4;
    
    let (applied, bypass_reason) = if orig_bytes < 256 {
        (false, Some("Output below 256 bytes threshold (MIN_BYTES)".into()))
    } else if meta.selected_profile.is_none() {
        (false, Some(meta.passthrough_reason.unwrap_or("unrecognized executable or command").to_string()))
    } else if model_bound_bytes >= orig_bytes {
        (false, Some("Rewritten output not smaller than original".into()))
    } else {
        (true, None)
    };
    
    // Check whether raw_output is duplicated in model-bound content
    let raw_duplicated = if orig_bytes > 0 && out.content.contains(raw_output) {
        true
    } else {
        false
    };
    
    DiagnosticTelemetry {
        command: cmd.to_string(),
        original_bytes: orig_bytes,
        original_tokens_est: orig_tokens,
        compressed_bytes: model_bound_bytes,
        compressed_tokens_est: compressed_tokens,
        selected_profile,
        applied,
        bypass_reason,
        model_bound_bytes,
        raw_output_duplicated: raw_duplicated,
        tool_name: tool_name.to_string(),
    }
}

#[test]
fn test_diagnostic_matrix_across_all_command_types() {
    let mut reports = Vec::new();

    // 1. Large cargo test output
    let mut large_cargo_test = String::from("running 42 tests\n");
    for i in 0..40 {
        large_cargo_test.push_str(&format!("test suite::test_{i} ... ok\n"));
    }
    large_cargo_test.push_str("test suite::test_broken_alpha ... FAILED\n");
    large_cargo_test.push_str("failures:\n\n---- suite::test_broken_alpha stdout ----\nassertion failed: `(left == right)`\n  left: `1`\n right: `2`\n");
    large_cargo_test.push_str(&"   stack trace line ...\n".repeat(30));
    large_cargo_test.push_str("\ntest result: FAILED. 41 passed; 1 failed; 0 ignored; finished in 1.42s\n");
    
    let rep_cargo = instrument_command("bash", "cargo test", &large_cargo_test, 101, false);
    assert!(rep_cargo.applied, "Cargo test must be compressed");
    assert_eq!(rep_cargo.selected_profile.as_deref(), Some("CargoTest"));
    assert!(!rep_cargo.raw_output_duplicated);
    reports.push(rep_cargo);

    // 2. Large git diff
    let mut large_git_diff = String::from("diff --git a/crates/agent/src/lib.rs b/crates/agent/src/lib.rs\nindex 1111..2222 100644\n--- a/crates/agent/src/lib.rs\n+++ b/crates/agent/src/lib.rs\n@@ -100,50 +100,50 @@\n");
    for i in 0..40 {
        large_git_diff.push_str(&format!(" context_line_{i}();\n"));
    }
    large_git_diff.push_str("-old_function_call();\n+new_function_call();\n");
    for i in 0..40 {
        large_git_diff.push_str(&format!(" trailing_context_{i}();\n"));
    }
    let rep_diff = instrument_command("bash", "git diff", &large_git_diff, 0, true);
    assert!(rep_diff.applied, "Git diff must be compressed");
    assert_eq!(rep_diff.selected_profile.as_deref(), Some("GitDiff"));
    assert!(!rep_diff.raw_output_duplicated);
    reports.push(rep_diff);

    // 3. npm test / Jest / Vitest output
    let mut npm_test_out = String::from("PASS src/utils.test.ts\nPASS src/auth.test.ts\nFAIL src/checkout.test.ts\n  ● Checkout › should calculate tax\n    expect(received).toBe(expected)\n    Expected: 10\n    Received: 12\n");
    for i in 0..30 {
        npm_test_out.push_str(&format!("      at src/checkout.test.ts:{i}:12\n"));
    }
    npm_test_out.push_str("\nTest Suites: 1 failed, 2 passed, 3 total\nTests: 1 failed, 20 passed, 21 total\nSnapshots: 0 total\nTime: 2.123 s\n");
    let rep_npm = instrument_command("bash", "npm test", &npm_test_out, 1, false);
    assert!(rep_npm.applied, "npm test must be compressed");
    assert_eq!(rep_npm.selected_profile.as_deref(), Some("JestVitest"));
    assert!(!rep_npm.raw_output_duplicated);
    reports.push(rep_npm);

    // 4. pytest failure output
    let mut pytest_out = String::from("============================= test session starts ==============================\ncollecting ... collected 50 items\n");
    for i in 0..40 {
        pytest_out.push_str(&format!("tests/test_{i}.py . [ 80%]\n"));
    }
    pytest_out.push_str("FAILED tests/test_payment.py::test_stripe_webhook - AssertionError: status 500 != 200\n");
    pytest_out.push_str("=================================== FAILURES ===================================\n");
    pytest_out.push_str("______________________________ test_stripe_webhook _____________________________\n");
    pytest_out.push_str("    def test_stripe_webhook():\n>       assert resp.status_code == 200\nE       AssertionError: assert 500 == 200\n");
    pytest_out.push_str("=========================== short test summary info ============================\n");
    pytest_out.push_str("FAILED tests/test_payment.py::test_stripe_webhook - AssertionError: assert 500 == 200\n");
    pytest_out.push_str("========================= 1 failed, 49 passed in 3.12s =========================\n");
    let rep_pytest = instrument_command("bash", "pytest tests/", &pytest_out, 1, false);
    assert!(rep_pytest.applied, "pytest must be compressed");
    assert_eq!(rep_pytest.selected_profile.as_deref(), Some("Pytest"));
    assert!(!rep_pytest.raw_output_duplicated);
    reports.push(rep_pytest);

    // 5. Unknown command (e.g. custom utility)
    let unknown_out = "my_custom_tool: record 1\nmy_custom_tool: record 2\n".repeat(20);
    let rep_unknown = instrument_command("bash", "./my_custom_tool --dump", &unknown_out, 0, true);
    assert!(!rep_unknown.applied, "Unknown command must be safely bypassed");
    assert_eq!(rep_unknown.selected_profile, None);
    assert!(rep_unknown.bypass_reason.is_some());
    reports.push(rep_unknown);

    // 6. Small output (< 256 bytes)
    let small_out = "hello world\n";
    let rep_small = instrument_command("bash", "git status", small_out, 0, true);
    assert!(!rep_small.applied, "Small output must be bypassed");
    assert!(rep_small.bypass_reason.as_deref().unwrap().contains("256"));
    reports.push(rep_small);

    // 7. SandboxedBash routing
    let sb = SandboxedBash::new(sandbox::Level::RestrictedEnv, None);
    assert_eq!(tools::Tool::name(&sb), "bash");
    let rep_sb = instrument_command("sandboxed_bash", "cargo test", &large_cargo_test, 101, false);
    assert!(rep_sb.applied);
    assert_eq!(rep_sb.selected_profile.as_deref(), Some("CargoTest"));
    reports.push(rep_sb);

    // 8. Non-shell tool verification (editor / list_dir / MCP)
    // Non-shell tools do NOT call format_command_output or process_and_compact
    let non_shell_editor = tools::Output::ok("file content updated at line 42");
    assert_eq!(non_shell_editor.observation, None);
    assert_eq!(non_shell_editor.raw_content, None);

    println!("\n=== DIAGNOSTIC TELEMETRY MATRIX ===");
    for r in &reports {
        println!("{}", serde_json::to_string_pretty(r).unwrap());
    }
}
