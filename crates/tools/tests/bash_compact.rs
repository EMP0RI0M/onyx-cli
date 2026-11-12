//! End-to-end proof that ONYX_COMPACT compresses real command output at the
//! Bash tool seam: run a genuinely large `ls -la`, then assert the returned
//! content is far smaller than raw while still carrying the file names and the
//! exit-code trailer. A second case runs with compression off (a command whose
//! output no formatter owns) to confirm passthrough.

use serde_json::json;
use std::path::PathBuf;
use tools::bash::Bash;
use tools::Tool;

fn scratch_dir() -> PathBuf {
    let d = std::env::temp_dir().join("onyx_bash_compact_it");
    std::fs::create_dir_all(&d).unwrap();
    // Seed enough long-format entries that raw output clears the 512-byte gate.
    for i in 0..40 {
        std::fs::write(d.join(format!("a_file_with_a_long_name_{i}.rs")), "x").unwrap();
    }
    d
}

#[test]
fn bash_compresses_ls_when_gate_enabled() {
    // The gate reads ONYX_COMPACT once per process; this test binary sets it
    // before any Bash call so compact_on() caches true.
    std::env::set_var("ONYX_COMPACT", "1");
    let dir = scratch_dir();
    let cmd = format!("ls -la {}", dir.display());
    let bash = Bash { cwd: None };
    let out = bash.execute(&json!({"command": cmd})).unwrap();

    // Raw listing would be ~40 lines of full mode/owner/size columns.
    assert!(out.content.contains("file(s),"), "no ls summary line: {}", out.content);
    assert!(out.content.contains("a_file_with_a_long_name_0.rs"), "name lost");
    assert!(out.content.ends_with("[exit 0]"), "trailer missing: {}", out.content);
    assert!(out.raw_content.as_ref().is_some_and(|raw| raw.len() > out.content.len()));
    assert_eq!(
        out.observation.as_ref().and_then(|obs| obs.kind),
        Some(tools::compress::FormatterKind::Ls)
    );
    // Compressed body must be materially shorter than a naive raw dump would be.
    assert!(out.content.len() < 40 * 60, "output not compressed: {} bytes", out.content.len());

    let (saved, _shipped) = tools::compress::gain();
    assert!(saved > 0, "compression accounting recorded no savings");
}

#[test]
fn bash_keeps_raw_failure_and_structured_observation() {
    let raw = format!(
        "error[E0425]: missing value\n  --> src/lib.rs:7:3\n{}\n",
        "x".repeat(300)
    );
    let (observation, compacted, savings) =
        tools::compress::process_and_compact("cargo check", &raw, None);
    assert!(matches!(savings, tools::compress::Savings::Compacted { .. }));
    assert!(compacted.contains("error [E0425]"));
    assert_eq!(
        observation.errors.first().and_then(|err| err.line),
        Some(7)
    );
}

#[test]
fn bash_passes_through_unknown_commands() {
    std::env::set_var("ONYX_COMPACT", "1");
    let bash = Bash { cwd: None };
    // `printf` is not a recognized formatter → output preserved verbatim.
    let big = "y".repeat(2000);
    let out = bash
        .execute(&json!({"command": format!("printf '{big}'")}))
        .unwrap();
    assert!(out.content.contains(&big), "unknown command was altered");
    assert!(out.content.ends_with("[exit 0]"));
}
