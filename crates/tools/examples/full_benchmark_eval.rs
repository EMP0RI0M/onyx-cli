use std::fs;
use std::io::Write;
use std::time::Instant;
use tools::compress::{self, Savings};

#[derive(Clone)]
struct Workload {
    name: &'static str,
    command: &'static str,
    raw: String,
    focus: Vec<String>,
    relevant_markers: Vec<&'static str>,
    unrelated_markers: Vec<&'static str>,
}

fn get_vm_hwm_kb() -> usize {
    if let Ok(status) = fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if line.starts_with("VmHWM:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(v) = parts[1].parse::<usize>() {
                        return v;
                    }
                }
            }
        }
    }
    0
}

fn build_workloads() -> Vec<Workload> {
    // 1. Large Cargo workspace check
    let mut cargo_check = String::new();
    for i in 0..150 {
        cargo_check.push_str(&format!(
            "   Compiling crate_{i} v0.1.0 (/root/crates/crate_{i})\n\
             error[E0308]: mismatched types\n  --> crates/target_crate/src/lib.rs:{}:9\n   |\n{} | let x: u32 = \"bad string literal\";\n   |         ---   ^^^^^^^^^^^^^^^^^^^^ expected `u32`, found `&str`\n   |\n",
            i + 15, i + 15
        ));
    }
    cargo_check.push_str("error: could not compile `crate_149` due to 150 previous errors\n");

    // 2. Cargo Clippy with mixed warnings and errors
    let mut clippy = String::new();
    for i in 0..120 {
        clippy.push_str(&format!(
            "warning: unused variable `unrelated_var_{i}`\n  --> crates/other_crate/src/lib.rs:{}:5\n   |\n{} | let unrelated_var_{i} = 1;\n   |     ^^^^^^^^^^^^^^^^^^ help: if this is intentional, prefix it with an underscore: `_unrelated_var_{i}`\n   |\n\
             error[E0425]: cannot find value `target_missing_func_{i}` in this scope\n  --> crates/target_crate/src/main.rs:{}:9\n   |\n{} | target_missing_func_{i}();\n   | ^^^^^^^^^^^^^^^^^^^^^ not found in this scope\n   |\n",
            i + 1, i + 1, i + 2, i + 2
        ));
    }

    // 3. Cargo test with failures across multiple crates
    let mut cargo_test = String::new();
    cargo_test.push_str("running 45 tests\n");
    for i in 0..40 {
        cargo_test.push_str(&format!("test crates::crate_{i}::test_ok_{i} ... ok\n"));
    }
    cargo_test.push_str("test crates::target_crate::test_critical_logic ... FAILED\n");
    cargo_test.push_str("test crates::other_crate::test_secondary_helper ... FAILED\n");
    for _ in 0..200 {
        cargo_test.push_str("verbose test log trace line: context data buffer dump [0x00, 0xAA, 0xFF]\n");
    }
    cargo_test.push_str("failures:\n---- crates::target_crate::test_critical_logic stdout ----\nthread 'test_critical_logic' panicked at crates/target_crate/tests/it.rs:44:5:\nassertion `left == right` failed\n  left: 42\n right: 99\n\n");
    cargo_test.push_str("---- crates::other_crate::test_secondary_helper stdout ----\nthread 'test_secondary_helper' panicked at crates/other_crate/tests/it.rs:12:3:\nunexpected none\n\n");
    cargo_test.push_str("failures:\n    crates::target_crate::test_critical_logic\n    crates::other_crate::test_secondary_helper\n\ntest result: FAILED. 40 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out;\n");

    // 4. Large Git diff
    let mut diff = String::new();
    for f in 0..25 {
        diff.push_str(&format!("diff --git a/src/module_{f}.rs b/src/module_{f}.rs\nindex 1111111..2222222 100644\n--- a/src/module_{f}.rs\n+++ b/src/module_{f}.rs\n@@ -1,50 +1,52 @@\n"));
        for l in 0..40 {
            diff.push_str(&format!(" unchanged_code_line_{l}_module_{f}();\n"));
        }
        diff.push_str("-let old_config_option = false;\n+let new_config_option = true;\n");
        for l in 40..80 {
            diff.push_str(&format!(" unchanged_trailing_line_{l}_module_{f}();\n"));
        }
    }

    // 5. Jest / Vitest failure output
    let mut jest = String::new();
    for s in 0..30 {
        jest.push_str(&format!("PASS test/unit/suite_{s}.test.ts (15 ms)\n"));
        for t in 0..5 {
            jest.push_str(&format!("  ✓ item {t} works\n"));
        }
    }
    jest.push_str("FAIL test/target/auth.test.ts (45 ms)\n  ✕ authenticates jwt token\n  ● authenticates jwt token\n    AssertionError: expected 'valid' to be 'expired'\n      at test/target/auth.test.ts:88:14\n");
    jest.push_str("FAIL test/other/analytics.test.ts (20 ms)\n  ✕ sends telemetry heartbeat\n  ● sends telemetry heartbeat\n    NetworkError: ECONNREFUSED 127.0.0.1:9099\n      at test/other/analytics.test.ts:15:9\n");
    jest.push_str("Test Suites: 2 failed, 30 passed, 32 total\nTests: 2 failed, 150 passed, 152 total\nSnapshots: 0 total\nTime: 2.14 s\n");

    // 6. Pytest failure output
    let mut pytest = String::new();
    for i in 0..120 {
        pytest.push_str(&format!("tests/test_unit_{i}.py ....\n"));
    }
    pytest.push_str("FAILED tests/test_other.py::test_secondary_service - KeyError: 'session'\n");
    pytest.push_str("FAILED tests/target_dir/test_core.py::test_reconciliation - assert False == True\n");
    pytest.push_str("=== FAILURES ===\n");
    pytest.push_str("___ test_reconciliation ___\ntests/target_dir/test_core.py:42: in test_reconciliation\n    assert balance == expected\nE   AssertionError: assert 100 == 250\n");
    pytest.push_str("___ test_secondary_service ___\ntests/test_other.py:12: in test_secondary_service\n    raise KeyError('session')\nE   KeyError: 'session'\n");
    pytest.push_str("=== 2 failed, 120 passed in 2.34s ===\n");

    // 7. Unrelated failures mixed with task-relevant failures
    let mut mixed = String::new();
    mixed.push_str("   Compiling common v0.1.0\n");
    mixed.push_str("error[E0425]: cannot find value `unrelated_val` in this scope\n  --> crates/unrelated/src/lib.rs:10:5\n   |\n10 |     unrelated_val();\n   |\n");
    mixed.push_str("error[E0308]: mismatched types in target service\n  --> crates/target_crate/src/service.rs:99:12\n   |\n99 |     return Err(CustomError::Timeout);\n   |            ^^^^^^^^^^^^^^^^^^^^^^^^^ expected `()`, found `Result<_, _>`\n   |\n");
    for i in 0..50 {
        mixed.push_str(&format!("warning: unused import `std::io::Error{i}`\n  --> crates/unrelated/src/unused.rs:{i}:5\n"));
    }
    mixed.push_str("error: could not compile `common` due to 2 previous errors\n");

    // 8. Malformed and truncated command output
    let mut malformed = String::new();
    malformed.push_str("error[E0599]: no method named `process_transaction` found for struct `crates/target_crate::Context`\n  --> crates/target_crate/src/handler.rs:50:18\n   |\n50 |         self.ctx.process_trans"); // truncated mid-line
    malformed.push_str(&" [TRUNCATED STREAM DATA ... \x1b[31m\x00".repeat(10));

    vec![
        Workload {
            name: "cargo-check-workspace",
            command: "cargo check",
            raw: cargo_check,
            focus: vec!["crates/target_crate".into()],
            relevant_markers: vec!["crates/target_crate/src/lib.rs", "error[E0308]"],
            unrelated_markers: vec!["crate_0", "crate_10"],
        },
        Workload {
            name: "cargo-clippy-mixed",
            command: "cargo clippy",
            raw: clippy,
            focus: vec!["crates/target_crate".into()],
            relevant_markers: vec!["crates/target_crate/src/main.rs", "error[E0425]"],
            unrelated_markers: vec!["crates/other_crate/src/lib.rs", "unrelated_var"],
        },
        Workload {
            name: "cargo-test-multi",
            command: "cargo test",
            raw: cargo_test,
            focus: vec!["crates/target_crate".into()],
            relevant_markers: vec!["test_critical_logic", "crates/target_crate/tests/it.rs"],
            unrelated_markers: vec!["test_secondary_helper", "crates/other_crate"],
        },
        Workload {
            name: "large-git-diff",
            command: "git diff",
            raw: diff,
            focus: vec!["src/module_0.rs".into(), "src/module_1.rs".into()],
            relevant_markers: vec!["src/module_0.rs", "+let new_config_option = true;"],
            unrelated_markers: vec!["src/module_24.rs"],
        },
        Workload {
            name: "jest-vitest",
            command: "npm test",
            raw: jest,
            focus: vec!["test/target/auth.test.ts".into()],
            relevant_markers: vec!["test/target/auth.test.ts", "authenticates jwt token"],
            unrelated_markers: vec!["test/other/analytics.test.ts", "telemetry heartbeat"],
        },
        Workload {
            name: "pytest",
            command: "pytest",
            raw: pytest,
            focus: vec!["tests/target_dir/test_core.py".into()],
            relevant_markers: vec!["tests/target_dir/test_core.py", "test_reconciliation"],
            unrelated_markers: vec!["tests/test_other.py", "test_secondary_service"],
        },
        Workload {
            name: "mixed-unrelated-task",
            command: "cargo check",
            raw: mixed,
            focus: vec!["crates/target_crate".into()],
            relevant_markers: vec!["crates/target_crate/src/service.rs", "CustomError::Timeout"],
            unrelated_markers: vec!["crates/unrelated/src/unused.rs"],
        },
        Workload {
            name: "malformed-truncated",
            command: "cargo check",
            raw: malformed,
            focus: vec!["crates/target_crate".into()],
            relevant_markers: vec!["crates/target_crate::Context", "crates/target_crate/src/handler.rs"],
            unrelated_markers: vec![],
        },
    ]
}

fn percentile(mut v: Vec<u128>, p: usize) -> u128 {
    v.sort_unstable();
    v[((v.len() - 1) * p) / 100]
}

#[derive(serde::Serialize)]
struct BenchmarkOutputRow {
    workload: String,
    group: String,
    original_bytes: usize,
    processed_bytes: usize,
    reduction_pct: f64,
    p50_us: u128,
    p95_us: u128,
    peak_memory_kb: usize,
    relevant_total: usize,
    relevant_retained: usize,
    unrelated_total: usize,
    unrelated_retained: usize,
    fn_count: usize, // false negative count (relevant dropped)
    fp_count: usize, // false positive count (unrelated erroneously kept if supposed to filter)
    output_text: String,
}

fn main() {
    let workloads = build_workloads();
    let mut results: Vec<BenchmarkOutputRow> = Vec::new();

    for w in &workloads {
        for group in &["A_raw", "B_deterministic", "C_architecture_aware"] {
            let mut timings = Vec::with_capacity(30);
            let mut output_str = String::new();

            for _ in 0..30 {
                let start = Instant::now();
                output_str = match *group {
                    "A_raw" => w.raw.clone(),
                    "B_deterministic" => compress::compact(w.command, &w.raw).0,
                    "C_architecture_aware" => compress::compact_with_architecture(w.command, &w.raw, Some(&w.focus)).0,
                    _ => unreachable!(),
                };
                timings.push(start.elapsed().as_micros());
            }

            let peak_mem = get_vm_hwm_kb();
            let proc_bytes = output_str.len();
            let reduction = if w.raw.len() > 0 {
                ((w.raw.len().saturating_sub(proc_bytes)) as f64 / w.raw.len() as f64) * 100.0
            } else {
                0.0
            };

            let rel_retained = w.relevant_markers.iter().filter(|m| output_str.contains(**m)).count();
            let unrel_retained = w.unrelated_markers.iter().filter(|m| output_str.contains(**m)).count();
            let fn_dropped = w.relevant_markers.len().saturating_sub(rel_retained);

            results.push(BenchmarkOutputRow {
                workload: w.name.to_string(),
                group: group.to_string(),
                original_bytes: w.raw.len(),
                processed_bytes: proc_bytes,
                reduction_pct: reduction,
                p50_us: percentile(timings.clone(), 50),
                p95_us: percentile(timings, 95),
                peak_memory_kb: peak_mem,
                relevant_total: w.relevant_markers.len(),
                relevant_retained: rel_retained,
                unrelated_total: w.unrelated_markers.len(),
                unrelated_retained: unrel_retained,
                fn_count: fn_dropped,
                fp_count: unrel_retained,
                output_text: output_str,
            });
        }
    }

    let json = serde_json::to_string_pretty(&results).expect("json serialize");
    let mut file = fs::File::create("/tmp/benchmark_phase71_results.json").expect("create file");
    file.write_all(json.as_bytes()).expect("write file");
    println!("BENCHMARK_COMPLETE: /tmp/benchmark_phase71_results.json");
}
