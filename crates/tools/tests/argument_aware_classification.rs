use std::time::Instant;
use tools::compress::{classify, parse_command_tokens, FormatterKind};

#[test]
fn test_quoted_pipes() {
    // Quoted pipes must NOT split the command segment
    let cmd = "echo \"hello | world\" | grep hello";
    let tokens = parse_command_tokens(cmd).expect("parsed tokens");
    assert_eq!(tokens, vec!["echo", "hello | world"]);

    let git_cmd = "git commit -m \"fix: pipeline | build issues\"";
    let tokens_git = parse_command_tokens(git_cmd).expect("git parsed");
    assert_eq!(tokens_git, vec!["git", "commit", "-m", "fix: pipeline | build issues"]);
}

#[test]
fn test_quoted_whitespace() {
    // Preserves whitespace inside quotes
    let cmd = "git diff \"path with spaces/file.rs\"";
    let tokens = parse_command_tokens(cmd).expect("parsed tokens");
    assert_eq!(tokens, vec!["git", "diff", "path with spaces/file.rs"]);
    assert_eq!(classify(cmd), Some(FormatterKind::GitDiff));
}

#[test]
fn test_escaped_quotes() {
    let cmd = "echo \"escaped \\\"quote\\\" inside\"";
    let tokens = parse_command_tokens(cmd).expect("parsed tokens");
    assert_eq!(tokens, vec!["echo", "escaped \"quote\" inside"]);
}

#[test]
fn test_environment_assignments() {
    // Leading environment variables stripped from executable resolution
    let cmd1 = "RUST_LOG=debug cargo test";
    assert_eq!(classify(cmd1), Some(FormatterKind::CargoTest));

    let cmd2 = "FOO=1 BAR=\"hello world\" _BAZ=test pytest tests/";
    assert_eq!(classify(cmd2), Some(FormatterKind::Pytest));

    // Non-leading assignment is an argument, not env assignment
    let cmd3 = "cargo test --package=foo";
    assert_eq!(classify(cmd3), Some(FormatterKind::CargoTest));
}

#[test]
fn test_multiple_shell_operators() {
    // Stops at operators
    let cmd1 = "cargo check && cargo test";
    let tokens1 = parse_command_tokens(cmd1).expect("tokens");
    assert_eq!(tokens1, vec!["cargo", "check"]);
    assert_eq!(classify(cmd1), Some(FormatterKind::CargoCheck));

    let cmd2 = "pytest; ls -la";
    let tokens2 = parse_command_tokens(cmd2).expect("tokens");
    assert_eq!(tokens2, vec!["pytest"]);
    assert_eq!(classify(cmd2), Some(FormatterKind::Pytest));
}

#[test]
fn test_command_substitutions() {
    // Command substitutions must safely fall back to raw/None (do not attempt to interpret)
    let cmd1 = "cargo test $(cat flags.txt)";
    assert_eq!(parse_command_tokens(cmd1), None);
    assert_eq!(classify(cmd1), None);

    let cmd2 = "pytest `which python`";
    assert_eq!(parse_command_tokens(cmd2), None);
    assert_eq!(classify(cmd2), None);
}

#[test]
fn test_empty_and_malformed_commands() {
    // Empty command
    assert_eq!(parse_command_tokens(""), None);
    assert_eq!(classify(""), None);

    // Whitespace only
    assert_eq!(parse_command_tokens("   \t  \n"), None);
    assert_eq!(classify("   "), None);

    // Malformed unclosed quote
    let malformed = "git commit -m \"unclosed string";
    assert_eq!(parse_command_tokens(malformed), None);
    assert_eq!(classify(malformed), None);
}

#[test]
fn test_unknown_commands() {
    // Unknown commands safely return None (passthrough)
    assert_eq!(classify("curl https://api.example.com"), None);
    assert_eq!(classify("tar -xzf archive.tar.gz"), None);
    assert_eq!(classify("docker run -d nginx"), None);
}

#[test]
fn test_existing_compression_behavior() {
    // Baseline formatters continue to match exactly as before
    assert_eq!(classify("git status"), Some(FormatterKind::GitStatus));
    assert_eq!(classify("git diff"), Some(FormatterKind::GitDiff));
    assert_eq!(classify("git log -n 5"), Some(FormatterKind::GitLog));
    assert_eq!(classify("cargo clippy --workspace"), Some(FormatterKind::CargoClippy));
    assert_eq!(classify("cargo check"), Some(FormatterKind::CargoCheck));
    assert_eq!(classify("cargo test"), Some(FormatterKind::CargoTest));
    assert_eq!(classify("npm test"), Some(FormatterKind::JestVitest));
    assert_eq!(classify("pnpm test"), Some(FormatterKind::JestVitest));
    assert_eq!(classify("vitest run"), Some(FormatterKind::JestVitest));
    assert_eq!(classify("pytest tests/"), Some(FormatterKind::Pytest));
    assert_eq!(classify("python -m pytest"), Some(FormatterKind::Pytest));
    assert_eq!(classify("ls -la"), Some(FormatterKind::Ls));
    assert_eq!(classify("rg search_term"), Some(FormatterKind::Grep));
    assert_eq!(classify("pip list"), Some(FormatterKind::PipList));
}

#[test]
fn test_classification_latency_benchmark() {
    let commands = [
        "git status",
        "FOO=1 BAR=\"val\" cargo clippy --all-targets",
        "echo \"hello | world\" | grep hello",
        "pytest -k \"test user and pass\"",
        "git diff HEAD~1 \"path with space/file.rs\"",
        "npm test -- -t \"component renders\"",
        "curl https://example.com/data.json",
    ];

    let mut latencies_ns = Vec::new();
    // Warm up
    for cmd in &commands {
        let _ = classify(cmd);
    }

    // Benchmark 1000 iterations per command
    for cmd in &commands {
        for _ in 0..1000 {
            let start = Instant::now();
            let _ = classify(cmd);
            latencies_ns.push(start.elapsed().as_nanos());
        }
    }

    latencies_ns.sort_unstable();
    let p50_ns = latencies_ns[latencies_ns.len() / 2];
    let p95_ns = latencies_ns[(latencies_ns.len() * 95) / 100];

    println!("\n=== Classification Latency Benchmark ===");
    println!("Total iterations: {}", latencies_ns.len());
    println!("p50 latency: {} ns ({:.3} µs)", p50_ns, p50_ns as f64 / 1000.0);
    println!("p95 latency: {} ns ({:.3} µs)", p95_ns, p95_ns as f64 / 1000.0);

    // Verify target under 3 µs on this environment
    assert!(p50_ns < 3000, "p50 must be under 3000 ns (3 µs), got {} ns", p50_ns);
    assert!(p95_ns < 5000, "p95 must be under 5000 ns (5 µs), got {} ns", p95_ns);
}
