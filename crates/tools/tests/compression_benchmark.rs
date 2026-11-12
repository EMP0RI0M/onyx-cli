use std::time::Instant;
use tools::compress::{self, Savings};

#[test]
fn benchmark_realistic_cargo_clippy_output() {
    let mut raw = String::new();
    for i in 0..100 {
        raw.push_str(&format!(
            "   Compiling package_{i} v0.1.0 (/root/pkg_{i})\n\
             warning: unused variable `temp_{i}`\n  --> crates/pkg_{i}/src/lib.rs:{}:12\n   |\n{} |     let temp_{i} = 42;\n   |\n\
             error[E0425]: cannot find value `unresolved_symbol_{i}` in this scope\n  --> crates/pkg_{i}/src/main.rs:{}:18\n   |\n{} |     let val = unresolved_symbol_{i};\n   |\n",
            i * 5 + 1, i * 5 + 1, i * 3 + 2, i * 3 + 2
        ));
    }
    raw.push_str("error: could not compile `package_99` due to 100 previous errors\n");

    let before_bytes = raw.len();
    assert!(before_bytes > 15_000, "Should be realistic multi-KB output");

    let start = Instant::now();
    let (compressed, savings) = compress::compact("cargo clippy", &raw);
    let latency = start.elapsed();

    assert!(latency.as_millis() < 50, "Latency must be well under 50ms, took {:?}", latency);
    assert!(matches!(savings, Savings::Compacted { .. }));
    let after_bytes = compressed.len();
    let reduction_pct = ((before_bytes - after_bytes) as f64 / before_bytes as f64) * 100.0;

    println!("\n=== Benchmark: Cargo Clippy Diagnostics ===");
    println!("Raw size: {} bytes", before_bytes);
    println!("Compacted size: {} bytes", after_bytes);
    println!("Reduction: {:.1}%", reduction_pct);
    println!("Latency: {:?}", latency);

    assert!(reduction_pct >= 50.0, "Expected at least 50% token reduction, got {:.1}%", reduction_pct);
    assert!(compressed.contains("error [E0425]"), "Must preserve compiler error signal");
}

#[test]
fn benchmark_large_git_diff() {
    let mut raw = String::new();
    for f in 0..15 {
        raw.push_str(&format!("diff --git a/crates/crate_{f}/src/lib.rs b/crates/crate_{f}/src/lib.rs\nindex 1234..5678 100644\n--- a/crates/crate_{f}/src/lib.rs\n+++ b/crates/crate_{f}/src/lib.rs\n@@ -10,30 +10,32 @@\n"));
        for line in 0..30 {
            raw.push_str(&format!(" unchanged_code_context_line_{line}();\n"));
        }
        raw.push_str("-let old_val = true;\n+let new_val = false;\n");
        for line in 30..60 {
            raw.push_str(&format!(" unchanged_footer_code_line_{line}();\n"));
        }
    }

    let before_bytes = raw.len();
    let start = Instant::now();
    let (compressed, savings) = compress::compact("git diff", &raw);
    let latency = start.elapsed();

    assert!(latency.as_millis() < 30, "Latency must be fast, took {:?}", latency);
    assert!(matches!(savings, Savings::Compacted { .. }));
    let after_bytes = compressed.len();
    let reduction_pct = ((before_bytes - after_bytes) as f64 / before_bytes as f64) * 100.0;

    println!("\n=== Benchmark: Large Git Diff ===");
    println!("Raw size: {} bytes", before_bytes);
    println!("Compacted size: {} bytes", after_bytes);
    println!("Reduction: {:.1}%", reduction_pct);
    println!("Latency: {:?}", latency);

    assert!(reduction_pct >= 60.0, "Expected >= 60% reduction on diffs, got {:.1}%", reduction_pct);
    assert!(compressed.contains("+let new_val = false;"));
    assert!(compressed.contains("-let old_val = true;"));
}

#[test]
fn benchmark_jest_vitest_failures() {
    let mut raw = String::new();
    for s in 0..20 {
        raw.push_str(&format!("PASS src/components/suite_{s}.test.tsx (12 ms)\n"));
        for t in 0..10 {
            raw.push_str(&format!("  ✓ renders test item {t} successfully\n"));
        }
    }
    raw.push_str("FAIL src/critical/checkout.test.tsx (35 ms)\n  ✕ processes cart item discount\n  ● processes cart item discount\n    AssertionError: expected 90 to be 100\n      at src/critical/checkout.test.tsx:55:12\n");
    raw.push_str("Tests: 1 failed, 200 passed, 201 total\n");

    let before_bytes = raw.len();
    let (compressed, savings) = compress::compact("npm test", &raw);
    assert!(matches!(savings, Savings::Compacted { .. }));
    let after_bytes = compressed.len();
    let reduction_pct = ((before_bytes - after_bytes) as f64 / before_bytes as f64) * 100.0;

    println!("\n=== Benchmark: Jest/Vitest Suite ===");
    println!("Raw size: {} bytes", before_bytes);
    println!("Compacted size: {} bytes", after_bytes);
    println!("Reduction: {:.1}%", reduction_pct);

    assert!(reduction_pct >= 70.0, "Expected >= 70% reduction on passing suites, got {:.1}%", reduction_pct);
    assert!(compressed.contains("FAIL: src/critical/checkout.test.tsx"));
    assert!(compressed.contains("AssertionError"));
}

#[test]
fn test_architecture_aware_focus_filtering() {
    let raw = format!(
        "   Compiling root v0.1.0\n\
         error[E0308]: mismatched types\n  --> crates/agent/src/session.rs:12:4\n   |\n12 |     true\n   |\n\
         error[E0425]: cannot find value\n  --> crates/tools/src/other.rs:99:1\n   |\n99 |     xyz\n   |\n{}\nerror: aborting",
        "filler log line\n".repeat(60)
    );

    let focus = vec!["crates/agent/src/session.rs".to_string()];
    let (compressed, _) = compress::compact_with_architecture("cargo check", &raw, Some(&focus));
    assert!(compressed.contains("🎯 [Task Focus] error [E0308]"), "Must tag focused modified file");
    assert!(compressed.contains("crates/agent/src/session.rs"));
}

#[test]
fn test_test_failures_prioritize_focus_without_dropping_other_failures() {
    let raw = format!(
        "FAIL tests/other.test.ts\n  ● other fails\nFAIL tests/changed.test.ts\n  ● changed fails\nTests: 2 failed\n{}",
        "noise\n".repeat(80)
    );
    let focus = vec!["tests/changed.test.ts".to_string()];
    let (compressed, _) = compress::compact_with_architecture("npm test", &raw, Some(&focus));
    assert!(compressed.contains("FAIL: [Task Focus] tests/changed.test.ts"));
    assert!(compressed.contains("FAIL: tests/other.test.ts"));
}
