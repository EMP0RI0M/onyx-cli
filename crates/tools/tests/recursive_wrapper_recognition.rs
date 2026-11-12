use std::time::Instant;
use tools::compress::{classify, classify_with_meta, unwrap_command_chain, FormatterKind};

#[test]
fn test_all_supported_wrappers_single_level() {
    // 1. sudo
    let meta = classify_with_meta("sudo -u root cargo test");
    assert_eq!(meta.selected_profile, Some(FormatterKind::CargoTest));
    assert_eq!(meta.recognized_executable, Some("cargo".into()));
    assert_eq!(meta.wrapper_chain, vec!["sudo"]);

    // 2. time
    let meta = classify_with_meta("time -p pytest tests/unit");
    assert_eq!(meta.selected_profile, Some(FormatterKind::Pytest));
    assert_eq!(meta.wrapper_chain, vec!["time"]);

    // 3. env
    let meta = classify_with_meta("env -i VAR=1 git status");
    assert_eq!(meta.selected_profile, Some(FormatterKind::GitStatus));
    assert_eq!(meta.wrapper_chain, vec!["env"]);

    // 4. poetry run
    let meta = classify_with_meta("poetry run pytest -k unit");
    assert_eq!(meta.selected_profile, Some(FormatterKind::Pytest));
    assert_eq!(meta.wrapper_chain, vec!["poetry run"]);

    // 5. uv run
    let meta = classify_with_meta("uv run pytest tests/unit");
    assert_eq!(meta.selected_profile, Some(FormatterKind::Pytest));
    assert_eq!(meta.wrapper_chain, vec!["uv run"]);

    // 6. pipenv run
    let meta = classify_with_meta("pipenv run pytest");
    assert_eq!(meta.selected_profile, Some(FormatterKind::Pytest));
    assert_eq!(meta.wrapper_chain, vec!["pipenv run"]);

    // 7. python -m
    let meta = classify_with_meta("python3 -u -m pytest");
    assert_eq!(meta.selected_profile, Some(FormatterKind::Pytest));
    assert_eq!(meta.wrapper_chain, vec!["python -m"]);

    // 8. pnpm exec & pnpm dlx
    let meta1 = classify_with_meta("pnpm exec vitest run");
    assert_eq!(meta1.selected_profile, Some(FormatterKind::JestVitest));
    assert_eq!(meta1.wrapper_chain, vec!["pnpm exec"]);

    let meta2 = classify_with_meta("pnpm dlx vitest");
    assert_eq!(meta2.selected_profile, Some(FormatterKind::JestVitest));
    assert_eq!(meta2.wrapper_chain, vec!["pnpm dlx"]);

    // 9. npx & bunx
    let meta_npx = classify_with_meta("npx -p vitest vitest run");
    assert_eq!(meta_npx.selected_profile, Some(FormatterKind::JestVitest));
    assert_eq!(meta_npx.wrapper_chain, vec!["npx"]);

    let meta_bunx = classify_with_meta("bunx vitest");
    assert_eq!(meta_bunx.selected_profile, Some(FormatterKind::JestVitest));
    assert_eq!(meta_bunx.wrapper_chain, vec!["bunx"]);

    // 10. docker exec & podman exec
    let meta_docker = classify_with_meta("docker exec -it -e DEBUG=1 app_container cargo clippy");
    assert_eq!(meta_docker.selected_profile, Some(FormatterKind::CargoClippy));
    assert_eq!(meta_docker.wrapper_chain, vec!["docker exec"]);

    let meta_podman = classify_with_meta("podman exec -w /app worker_pod cargo check");
    assert_eq!(meta_podman.selected_profile, Some(FormatterKind::CargoCheck));
    assert_eq!(meta_podman.wrapper_chain, vec!["podman exec"]);
}

#[test]
fn test_nested_wrapper_combinations() {
    // sudo + docker exec + cargo test
    let cmd1 = "sudo docker exec -it app cargo test --lib";
    let meta1 = classify_with_meta(cmd1);
    assert_eq!(meta1.selected_profile, Some(FormatterKind::CargoTest));
    assert_eq!(meta1.wrapper_chain, vec!["sudo", "docker exec"]);

    // time + env + poetry run + pytest
    let cmd2 = "time env RUST_LOG=info poetry run pytest tests/";
    let meta2 = classify_with_meta(cmd2);
    assert_eq!(meta2.selected_profile, Some(FormatterKind::Pytest));
    assert_eq!(meta2.wrapper_chain, vec!["time", "env", "poetry run"]);

    // sudo + env + uv run + python -m + pytest (4 levels)
    let cmd3 = "sudo env FOO=1 uv run python3 -m pytest";
    let meta3 = classify_with_meta(cmd3);
    assert_eq!(meta3.selected_profile, Some(FormatterKind::Pytest));
    assert_eq!(meta3.wrapper_chain, vec!["sudo", "env", "uv run", "python -m"]);
}

#[test]
fn test_recursion_depth_limit() {
    // 6 nested sudo wrappers (exceeds MAX_WRAPPER_DEPTH of 5)
    let cmd = "sudo sudo sudo sudo sudo sudo cargo check";
    let meta = classify_with_meta(cmd);
    assert_eq!(meta.selected_profile, None);
    assert_eq!(meta.passthrough_reason, Some("maximum wrapper recursion depth (5) exceeded"));

    // 5 nested wrappers (boundary: exactly 5 allowed)
    let cmd5 = "sudo sudo sudo sudo sudo cargo check";
    let meta5 = classify_with_meta(cmd5);
    assert_eq!(meta5.selected_profile, Some(FormatterKind::CargoCheck));
    assert_eq!(meta5.wrapper_chain.len(), 5);
}

#[test]
fn test_unknown_wrappers_and_make_just_safe_passthrough() {
    // make test, just test, nx run must NOT be guessed
    let meta_make = classify_with_meta("make test");
    assert_eq!(meta_make.selected_profile, None);
    assert_eq!(meta_make.recognized_executable, Some("make".into()));
    assert_eq!(meta_make.passthrough_reason, Some("unrecognized underlying executable"));

    let meta_just = classify_with_meta("just check");
    assert_eq!(meta_just.selected_profile, None);

    let meta_nx = classify_with_meta("nx run app:test");
    assert_eq!(meta_nx.selected_profile, None);

    let meta_turbo = classify_with_meta("turbo run test");
    assert_eq!(meta_turbo.selected_profile, None);

    let meta_bazel = classify_with_meta("bazel test //...");
    assert_eq!(meta_bazel.selected_profile, None);
}

#[test]
fn test_quoted_arguments_and_adversarial_flags() {
    // Argument containing wrapper name as string or flag value
    let cmd = "git commit -m \"sudo poetry run pytest\"";
    let meta = classify_with_meta(cmd);
    assert_eq!(meta.recognized_executable, Some("git".into()));
    assert_eq!(meta.selected_profile, None); // git commit is not status/diff/log

    // Docker exec with complex quoted env
    let cmd2 = "docker exec -e GREETING=\"hello world\" container_1 pytest -k \"test user\"";
    let meta2 = classify_with_meta(cmd2);
    assert_eq!(meta2.selected_profile, Some(FormatterKind::Pytest));
    assert_eq!(meta2.recognized_executable, Some("pytest".into()));
}

#[test]
fn test_performance_benchmark() {
    let commands = [
        "git status",
        "sudo -u deploy cargo test --workspace",
        "docker exec -it app pnpm exec vitest run",
        "time env FOO=1 poetry run pytest -k unit",
        "sudo env A=1 B=2 uv run python3 -m pytest tests/",
        "make test",
        "cargo clippy",
    ];

    println!("\n=== Phase 9.2 Recursive Unwrapping Benchmark ===");
    for cmd in &commands {
        // Warm up
        for _ in 0..100 {
            let _ = classify(cmd);
        }
        let batch_size = 50;
        let iters = 200;
        let mut batch_latencies = Vec::with_capacity(iters);

        for _ in 0..iters {
            let start = Instant::now();
            for _ in 0..batch_size {
                let _ = classify(cmd);
            }
            let avg_ns = start.elapsed().as_nanos() / batch_size as u128;
            batch_latencies.push(avg_ns);
        }

        batch_latencies.sort_unstable();
        let c_p50 = batch_latencies[iters / 2];
        let c_p95 = batch_latencies[(iters * 95) / 100];
        println!("{:<48} | p50: {:>4} ns ({:.3} µs) | p95: {:>4} ns ({:.3} µs)", cmd, c_p50, c_p50 as f64 / 1000.0, c_p95, c_p95 as f64 / 1000.0);

        // Target: p50 under 2 µs, p95 under 5 µs
        assert!(c_p50 < 2000, "Single command p50 must be under 2000 ns (2 µs), got {} ns", c_p50);
        assert!(c_p95 < 5000, "Single command p95 must be under 5000 ns (5 µs), got {} ns", c_p95);
    }
}
