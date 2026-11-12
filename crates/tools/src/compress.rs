//! Command-output compression — Architecture-Aware Unified Output Processing.
//!
//! Clean-room, Rust-native output filter and structured observation extractor.
//!
//! Intercepts verbose outputs from developer tools (cargo check/clippy/test,
//! git status/diff/log, npm/pnpm/yarn, vitest/jest, pytest) and distills them
//! into concise signal: error diagnostics, failures, stack traces, and hunk summaries,
//! while eliminating repetitive boilerplate and successful noise.
//!
//! Normalizes command output into a reusable `CommandObservation` so both
//! the model context and autonomous diagnostic engines parse it once without reparsing.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Session-wide telemetry accounting.
static COMPACTED_BYTES: AtomicU64 = AtomicU64::new(0);
static PASSTHROUGH_BYTES: AtomicU64 = AtomicU64::new(0);
static ESTIMATED_TOKENS_SAVED: AtomicU64 = AtomicU64::new(0);
static COMPACT_CALLS: AtomicU64 = AtomicU64::new(0);
static TOTAL_LATENCY_US: AtomicU64 = AtomicU64::new(0);
static LAST_FORMATTER: AtomicU64 = AtomicU64::new(u64::MAX);

/// Detailed telemetry snapshot for reporting and auditing.
#[derive(Debug, Clone, PartialEq)]
pub struct GainReport {
    pub raw_bytes: u64,
    pub compacted_bytes: u64,
    pub bytes_saved: u64,
    pub reduction_pct: f64,
    pub estimated_tokens_saved: u64,
    pub calls: u64,
    pub avg_latency_us: u64,
    pub formatter: Option<FormatterKind>,
}

pub fn record(savings: Savings) {
    let formatter = match savings {
        Savings::Compacted { formatter, .. } => Some(formatter),
        Savings::Unchanged(_) | Savings::Small(_) => None,
    };
    record_with_formatter(savings, formatter);
}

/// Record compression telemetry while retaining formatter attribution for
/// callers that kept the output verbatim or below the size threshold.
pub fn record_with_formatter(savings: Savings, formatter: Option<FormatterKind>) {
    if let Some(formatter) = formatter {
        LAST_FORMATTER.store(formatter.index() as u64, Ordering::Relaxed);
    }
    match savings {
        Savings::Compacted { before, after, latency_us, .. } => {
            let saved = before.saturating_sub(after) as u64;
            COMPACTED_BYTES.fetch_add(saved, Ordering::Relaxed);
            PASSTHROUGH_BYTES.fetch_add(after as u64, Ordering::Relaxed);
            // Heuristic estimation: ~4 chars per token for typical code/tool output
            let tokens = saved / 4;
            ESTIMATED_TOKENS_SAVED.fetch_add(tokens, Ordering::Relaxed);
            COMPACT_CALLS.fetch_add(1, Ordering::Relaxed);
            TOTAL_LATENCY_US.fetch_add(latency_us, Ordering::Relaxed);
        }
        Savings::Unchanged(n) | Savings::Small(n) => {
            PASSTHROUGH_BYTES.fetch_add(n as u64, Ordering::Relaxed);
        }
    }
}

/// Reset telemetry metrics (useful in benchmarks or tests).
pub fn reset_telemetry() {
    COMPACTED_BYTES.store(0, Ordering::Relaxed);
    PASSTHROUGH_BYTES.store(0, Ordering::Relaxed);
    ESTIMATED_TOKENS_SAVED.store(0, Ordering::Relaxed);
    COMPACT_CALLS.store(0, Ordering::Relaxed);
    TOTAL_LATENCY_US.store(0, Ordering::Relaxed);
    LAST_FORMATTER.store(u64::MAX, Ordering::Relaxed);
}

/// Total bytes removed from tool output this session, and total bytes that were
/// actually shipped to the model (post-compression). Backward compatible tuple.
pub fn gain() -> (u64, u64) {
    (
        COMPACTED_BYTES.load(Ordering::Relaxed),
        PASSTHROUGH_BYTES.load(Ordering::Relaxed),
    )
}

/// Comprehensive telemetry report with reduction percentage and latency.
pub fn gain_report() -> GainReport {
    let saved = COMPACTED_BYTES.load(Ordering::Relaxed);
    let passed = PASSTHROUGH_BYTES.load(Ordering::Relaxed);
    let tokens = ESTIMATED_TOKENS_SAVED.load(Ordering::Relaxed);
    let calls = COMPACT_CALLS.load(Ordering::Relaxed);
    let total_lat = TOTAL_LATENCY_US.load(Ordering::Relaxed);
    let raw = saved + passed;
    let reduction_pct = if raw > 0 {
        (saved as f64 / raw as f64) * 100.0
    } else {
        0.0
    };
    let avg_latency_us = if calls > 0 {
        total_lat / calls
    } else {
        0
    };

    GainReport {
        raw_bytes: raw,
        compacted_bytes: passed,
        bytes_saved: saved,
        reduction_pct,
        estimated_tokens_saved: tokens,
        calls,
        avg_latency_us,
        formatter: formatter_from_index(LAST_FORMATTER.load(Ordering::Relaxed) as usize),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Savings {
    /// Output was rewritten smaller. before >= after always holds.
    Compacted {
        before: usize,
        after: usize,
        latency_us: u64,
        formatter: FormatterKind,
    },
    /// Formatter chose to keep the output verbatim (safety fallback).
    Unchanged(usize),
    /// Output too small to be worth compressing.
    Small(usize),
}

/// Minimum output size before running formatters.
const MIN_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatterKind {
    GitStatus,
    GitDiff,
    GitLog,
    CargoTest,
    CargoCheck,
    CargoClippy,
    NpmYarn,
    JestVitest,
    Pytest,
    Ls,
    Grep,
    PipList,
}

impl FormatterKind {
    fn index(self) -> usize {
        match self {
            Self::GitStatus => 0,
            Self::GitDiff => 1,
            Self::GitLog => 2,
            Self::CargoTest => 3,
            Self::CargoCheck => 4,
            Self::CargoClippy => 5,
            Self::NpmYarn => 6,
            Self::JestVitest => 7,
            Self::Pytest => 8,
            Self::Ls => 9,
            Self::Grep => 10,
            Self::PipList => 11,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::GitStatus => "git status",
            Self::GitDiff => "git diff",
            Self::GitLog => "git log",
            Self::CargoTest => "cargo test",
            Self::CargoCheck => "cargo check",
            Self::CargoClippy => "cargo clippy",
            Self::NpmYarn => "npm/yarn",
            Self::JestVitest => "jest/vitest",
            Self::Pytest => "pytest",
            Self::Ls => "ls",
            Self::Grep => "grep/rg",
            Self::PipList => "pip list",
        }
    }

}

fn formatter_from_index(index: usize) -> Option<FormatterKind> {
    Some(match index {
        0 => FormatterKind::GitStatus,
        1 => FormatterKind::GitDiff,
        2 => FormatterKind::GitLog,
        3 => FormatterKind::CargoTest,
        4 => FormatterKind::CargoCheck,
        5 => FormatterKind::CargoClippy,
        6 => FormatterKind::NpmYarn,
        7 => FormatterKind::JestVitest,
        8 => FormatterKind::Pytest,
        9 => FormatterKind::Ls,
        10 => FormatterKind::Grep,
        11 => FormatterKind::PipList,
        _ => return None,
    })
}

/// Unified Structured Observation parsed from command output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandObservation {
    pub kind: Option<FormatterKind>,
    pub summary_line: String,
    pub errors: Vec<DiagnosticEntry>,
    pub warnings: Vec<DiagnosticEntry>,
    pub failed_tests: Vec<String>,
    pub passed_tests_count: usize,
    pub failed_tests_count: usize,
    pub files_mentioned: Vec<String>,
    pub raw_preserved: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiagnosticEntry {
    pub file: Option<String>,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub code: Option<String>,
    pub message: String,
    pub snippet: Option<String>,
    pub suggestion: Option<String>,
}

/// Compress the output of a shell command.
pub fn compact(cmd: &str, out: &str) -> (String, Savings) {
    compact_with_architecture(cmd, out, None)
}

/// Architecture-aware compression that prioritizes modified/focus files.
pub fn compact_with_architecture(
    cmd: &str,
    out: &str,
    focus_files: Option<&[String]>,
) -> (String, Savings) {
    let (_, compacted, savings) = process_and_compact(cmd, out, focus_files);
    (compacted, savings)
}

/// Parse and compact a command result once, returning the observation for
/// downstream diagnostics and checkpoint consumers.
pub fn process_and_compact(
    cmd: &str,
    out: &str,
    focus_files: Option<&[String]>,
) -> (CommandObservation, String, Savings) {
    let start = Instant::now();
    let before = out.len();
    if before < MIN_BYTES {
        return (
            CommandObservation {
                kind: classify(cmd),
                raw_preserved: true,
                ..Default::default()
            },
            out.to_string(),
            Savings::Small(before),
        );
    }

    let (obs, rewritten) = process_observation(cmd, out, focus_files);

    if rewritten == out || rewritten.is_empty() {
        (obs, out.to_string(), Savings::Unchanged(before))
    } else {
        let after = rewritten.len();
        if after > before {
            (obs, out.to_string(), Savings::Unchanged(before))
        } else {
            let latency_us = start.elapsed().as_micros() as u64;
            let formatter = obs.kind.unwrap_or(FormatterKind::GitStatus);
            (
                obs,
                rewritten,
                Savings::Compacted {
                    before,
                    after,
                    latency_us,
                    formatter,
                },
            )
        }
    }
}

/// Parse command output once into a reusable structured observation.
pub fn process_observation(
    cmd: &str,
    out: &str,
    focus_files: Option<&[String]>,
) -> (CommandObservation, String) {
    let fmt = classify(cmd);
    match fmt {
        Some(FormatterKind::GitStatus) => {
            let text = git_status(out);
            let obs = CommandObservation {
                kind: Some(FormatterKind::GitStatus),
                summary_line: text.lines().next().unwrap_or("").to_string(),
                ..Default::default()
            };
            (obs, text)
        }
        Some(FormatterKind::GitDiff) => git_diff(out, focus_files),
        Some(FormatterKind::GitLog) => git_log(out),
        Some(FormatterKind::CargoTest) => cargo_test(out),
        Some(FormatterKind::CargoCheck) | Some(FormatterKind::CargoClippy) => {
            cargo_diagnostics(out, fmt.unwrap(), focus_files)
        }
        Some(FormatterKind::NpmYarn) => npm_yarn(out),
        Some(FormatterKind::JestVitest) => jest_vitest(out, focus_files),
        Some(FormatterKind::Pytest) => pytest(out, focus_files),
        Some(FormatterKind::Ls) => {
            let text = ls(out);
            (
                CommandObservation {
                    kind: Some(FormatterKind::Ls),
                    summary_line: text.lines().next().unwrap_or("").to_string(),
                    ..Default::default()
                },
                text,
            )
        }
        Some(FormatterKind::Grep) => {
            let text = grep(out);
            (
                CommandObservation {
                    kind: Some(FormatterKind::Grep),
                    summary_line: "grep matches".to_string(),
                    ..Default::default()
                },
                text,
            )
        }
        Some(FormatterKind::PipList) => {
            let text = grouped_lines(out, "pip");
            (
                CommandObservation {
                    kind: Some(FormatterKind::PipList),
                    summary_line: text.lines().next().unwrap_or("").to_string(),
                    ..Default::default()
                },
                text,
            )
        }
        None => (
            CommandObservation {
                kind: None,
                raw_preserved: true,
                ..Default::default()
            },
            out.to_string(),
        ),
    }
}

/// Helper to check if a token is a POSIX environment variable assignment (e.g. `FOO=bar`, `_VAR="1"`).
fn is_env_assignment(tok: &str) -> bool {
    if let Some((k, _)) = tok.split_once('=') {
        if !k.is_empty() {
            let mut chars = k.chars();
            let first = chars.next().unwrap();
            if (first.is_ascii_alphabetic() || first == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return true;
            }
        }
    }
    false
}

/// Tokenize command into tokens of the first pipeline segment in a quote-aware, argument-aware manner.
/// Returns None if syntax is malformed (unclosed quotes), command substitution is detected, or no valid tokens exist.
pub fn parse_command_tokens(cmd: &str) -> Option<Vec<String>> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut lexer = shlex::Shlex::new(trimmed);
    let mut tokens: Vec<String> = Vec::with_capacity(8);

    while let Some(tok) = lexer.next() {
        // Stop at actual shell pipeline or command separator operators
        if tok == "|" || tok == "||" || tok == ";" || tok == "&&" || tok == "&" {
            break;
        }
        // If shlex absorbed a trailing semicolon like `pytest;`, strip it and terminate segment
        if let Some(stripped) = tok.strip_suffix(';') {
            if !stripped.is_empty() {
                tokens.push(stripped.to_string());
            }
            break;
        }
        // If command contains command substitution ($(...) or `...`), fall back to raw/None
        if tok.contains("$(") || tok.contains('`') {
            return None;
        }
        tokens.push(tok);
    }

    // If lexer encountered an error (such as an unclosed quotation mark), fall back safely
    if lexer.had_error || tokens.is_empty() {
        None
    } else {
        Some(tokens)
    }
}

/// Detailed classification metadata explaining how a command was resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassificationMeta {
    pub original_command: String,
    pub recognized_executable: Option<String>,
    pub wrapper_chain: Vec<String>,
    pub selected_profile: Option<FormatterKind>,
    pub method: &'static str,
    pub confidence: f64,
    pub passthrough_reason: Option<&'static str>,
}

/// Bounded recursive unwrap result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnwrappedCommand {
    pub executable: String,
    pub arguments: Vec<String>,
    pub wrapper_chain: Vec<String>,
    pub recursion_depth: usize,
}

const MAX_WRAPPER_DEPTH: usize = 5;

/// Unwraps a known execution wrapper from a token slice.
/// Returns Ok(Some((remaining_tokens, wrapper_name))) if unwrapped,
/// Ok(None) if the command is not a known wrapper (terminal executable reached),
/// Err(reason) if malformed or unsupported syntax encountered.
fn unwrap_step(tokens: &[String]) -> Result<Option<(&[String], String)>, &'static str> {
    if tokens.is_empty() {
        return Ok(None);
    }

    // Skip leading environment variable assignments
    let mut idx = 0;
    while idx < tokens.len() && is_env_assignment(&tokens[idx]) {
        idx += 1;
    }

    if idx >= tokens.len() {
        return Ok(None);
    }

    let exe = tokens[idx].rsplit('/').next().unwrap_or(&tokens[idx]);
    let mut args_idx = idx + 1;

    match exe {
        "sudo" => {
            let wrapper_desc = "sudo".to_string();
            while args_idx < tokens.len() {
                let arg = &tokens[args_idx];
                if arg == "--" {
                    args_idx += 1;
                    break;
                }
                if arg.starts_with("--") {
                    if arg.contains('=') {
                        args_idx += 1;
                        continue;
                    }
                    if arg == "--user" || arg == "--group" || arg == "--prompt" || arg == "--close-from" || arg == "--directory" {
                        args_idx += 2;
                        continue;
                    }
                    args_idx += 1;
                    continue;
                }
                if arg.starts_with('-') && arg != "-" {
                    // Short options with arguments: -u user, -g group, -p prompt, -C fd, -D dir, -R dir, -T timeout
                    if arg.len() == 2 && matches!(arg.chars().nth(1).unwrap(), 'u' | 'g' | 'p' | 'C' | 'D' | 'R' | 'T') {
                        args_idx += 2;
                        continue;
                    }
                    args_idx += 1;
                    continue;
                }
                break;
            }
            if args_idx < tokens.len() {
                Ok(Some((&tokens[args_idx..], wrapper_desc)))
            } else {
                Err("sudo without target command")
            }
        }
        "time" => {
            while args_idx < tokens.len() {
                let arg = &tokens[args_idx];
                if arg == "--" {
                    args_idx += 1;
                    break;
                }
                if arg.starts_with('-') && arg != "-" {
                    if arg == "-f" || arg == "-o" || arg == "--format" || arg == "--output" {
                        args_idx += 2;
                        continue;
                    }
                    args_idx += 1;
                    continue;
                }
                break;
            }
            if args_idx < tokens.len() {
                Ok(Some((&tokens[args_idx..], "time".to_string())))
            } else {
                Err("time without target command")
            }
        }
        "env" => {
            while args_idx < tokens.len() {
                let arg = &tokens[args_idx];
                if arg == "--" {
                    args_idx += 1;
                    break;
                }
                if arg == "-u" || arg == "--unset" || arg == "-C" || arg == "--chdir" {
                    args_idx += 2;
                    continue;
                }
                if arg.starts_with('-') && arg != "-" {
                    args_idx += 1;
                    continue;
                }
                if is_env_assignment(arg) {
                    args_idx += 1;
                    continue;
                }
                break;
            }
            if args_idx < tokens.len() {
                Ok(Some((&tokens[args_idx..], "env".to_string())))
            } else {
                Err("env without target command")
            }
        }
        "poetry" => {
            // poetry [-C dir] [--directory dir] [-v] [-q] run <cmd>
            let mut saw_run = false;
            while args_idx < tokens.len() {
                let arg = &tokens[args_idx];
                if arg == "run" {
                    saw_run = true;
                    args_idx += 1;
                    break;
                }
                if arg == "-C" || arg == "--directory" {
                    args_idx += 2;
                    continue;
                }
                if arg.starts_with('-') {
                    args_idx += 1;
                    continue;
                }
                // Non-option token before 'run' means it's another poetry subcommand (e.g. poetry install)
                break;
            }
            if saw_run && args_idx < tokens.len() {
                Ok(Some((&tokens[args_idx..], "poetry run".to_string())))
            } else {
                Ok(None)
            }
        }
        "uv" => {
            // uv run [--project path] [--directory dir] [-v] <cmd>
            let mut saw_run = false;
            while args_idx < tokens.len() {
                let arg = &tokens[args_idx];
                if arg == "run" {
                    saw_run = true;
                    args_idx += 1;
                    break;
                }
                if arg == "-C" || arg == "--directory" || arg == "--project" {
                    args_idx += 2;
                    continue;
                }
                if arg.starts_with('-') {
                    args_idx += 1;
                    continue;
                }
                break;
            }
            if saw_run {
                while args_idx < tokens.len() {
                    let arg = &tokens[args_idx];
                    if arg == "--" {
                        args_idx += 1;
                        break;
                    }
                    if arg == "--project" || arg == "--directory" || arg == "--package" {
                        args_idx += 2;
                        continue;
                    }
                    if arg.starts_with('-') && !arg.contains('=') && (arg.starts_with("-p") || arg.starts_with("-C")) {
                        args_idx += 2;
                        continue;
                    }
                    if arg.starts_with('-') {
                        args_idx += 1;
                        continue;
                    }
                    break;
                }
                if args_idx < tokens.len() {
                    return Ok(Some((&tokens[args_idx..], "uv run".to_string())));
                }
            }
            Ok(None)
        }
        "pipenv" => {
            // pipenv run <cmd>
            let mut saw_run = false;
            while args_idx < tokens.len() {
                let arg = &tokens[args_idx];
                if arg == "run" {
                    saw_run = true;
                    args_idx += 1;
                    break;
                }
                if arg.starts_with('-') {
                    args_idx += 1;
                    continue;
                }
                break;
            }
            if saw_run && args_idx < tokens.len() {
                Ok(Some((&tokens[args_idx..], "pipenv run".to_string())))
            } else {
                Ok(None)
            }
        }
        "pnpm" => {
            // pnpm exec <cmd> or pnpm dlx <cmd>
            if args_idx < tokens.len() {
                let sub = &tokens[args_idx];
                if sub == "exec" || sub == "dlx" {
                    args_idx += 1;
                    while args_idx < tokens.len() && tokens[args_idx].starts_with('-') {
                        if tokens[args_idx] == "--" {
                            args_idx += 1;
                            break;
                        }
                        args_idx += 1;
                    }
                    if args_idx < tokens.len() {
                        return Ok(Some((&tokens[args_idx..], format!("pnpm {sub}"))));
                    }
                }
            }
            Ok(None)
        }
        "npx" | "bunx" => {
            let wrapper = exe.to_string();
            while args_idx < tokens.len() {
                let arg = &tokens[args_idx];
                if arg == "--" {
                    args_idx += 1;
                    break;
                }
                if arg == "-p" || arg == "--package" {
                    args_idx += 2;
                    continue;
                }
                if arg.starts_with('-') {
                    args_idx += 1;
                    continue;
                }
                break;
            }
            if args_idx < tokens.len() {
                Ok(Some((&tokens[args_idx..], wrapper)))
            } else {
                Err("npx/bunx without target package/command")
            }
        }
        "docker" | "podman" => {
            // docker/podman exec [flags] <container> <cmd>
            let engine = exe.to_string();
            if args_idx < tokens.len() && tokens[args_idx] == "exec" {
                args_idx += 1;
                while args_idx < tokens.len() {
                    let arg = &tokens[args_idx];
                    if arg == "--" {
                        args_idx += 1;
                        break;
                    }
                    if arg == "-e" || arg == "--env" || arg == "-u" || arg == "--user" || arg == "-w" || arg == "--workdir" {
                        args_idx += 2;
                        continue;
                    }
                    if arg.starts_with('-') {
                        args_idx += 1;
                        continue;
                    }
                    break;
                }
                // The next token is the container name/id
                if args_idx < tokens.len() {
                    let _container = &tokens[args_idx];
                    args_idx += 1;
                    if args_idx < tokens.len() {
                        return Ok(Some((&tokens[args_idx..], format!("{engine} exec"))));
                    }
                }
            }
            Ok(None)
        }
        "python" | "python3" => {
            // python [flags] -m <module> <args...>
            let mut saw_m = false;
            while args_idx < tokens.len() {
                let arg = &tokens[args_idx];
                if arg == "-m" {
                    saw_m = true;
                    args_idx += 1;
                    break;
                }
                if arg == "-W" || arg == "-X" {
                    args_idx += 2;
                    continue;
                }
                if arg.starts_with('-') {
                    args_idx += 1;
                    continue;
                }
                break;
            }
            if saw_m && args_idx < tokens.len() {
                // Return module as the target executable
                Ok(Some((&tokens[args_idx..], "python -m".to_string())))
            } else {
                Ok(None)
            }
        }
        _ => Ok(None),
    }
}

/// Recursively unwrap known command execution wrappers up to MAX_WRAPPER_DEPTH (5).
/// Returns Ok(Some(UnwrappedCommand)) if unwrapped, Ok(None) if empty or non-executable,
/// or Err(reason) if recursion exceeded or malformed.
pub fn unwrap_command_chain(cmd: &str) -> Result<Option<UnwrappedCommand>, &'static str> {
    let raw_tokens = match parse_command_tokens(cmd) {
        Some(t) => t,
        None => return Ok(None),
    };

    let mut current_slice = raw_tokens.as_slice();
    let mut wrapper_chain = Vec::new();
    let mut depth = 0;

    loop {
        match unwrap_step(current_slice) {
            Ok(Some((remaining, wrapper))) => {
                if depth >= MAX_WRAPPER_DEPTH {
                    return Err("maximum wrapper recursion depth (5) exceeded");
                }
                wrapper_chain.push(wrapper);
                current_slice = remaining;
                depth += 1;
            }
            Ok(None) => break,
            Err(e) => return Err(e),
        }
    }

    // Skip any remaining leading env vars on the final target
    let mut idx = 0;
    while idx < current_slice.len() && is_env_assignment(&current_slice[idx]) {
        idx += 1;
    }

    if idx >= current_slice.len() {
        return Ok(None);
    }

    let executable = current_slice[idx].rsplit('/').next().unwrap_or(&current_slice[idx]).to_string();
    let arguments = current_slice[idx + 1..].to_vec();

    Ok(Some(UnwrappedCommand {
        executable,
        arguments,
        wrapper_chain,
        recursion_depth: depth,
    }))
}

/// Inspect and classify command, returning full explainable metadata.
pub fn classify_with_meta(cmd: &str) -> ClassificationMeta {
    let unwrap_res = unwrap_command_chain(cmd);

    let (unwrapped, err_reason) = match unwrap_res {
        Ok(Some(u)) => (Some(u), None),
        Ok(None) => (None, Some("empty, malformed, or command substitution")),
        Err(e) => (None, Some(e)),
    };

    if let Some(u) = unwrapped {
        let exe = u.executable.as_str();
        let rest = u.arguments.as_slice();

        let profile = match exe {
            "git" => {
                let sub = rest.iter().find(|a| !a.starts_with('-'));
                match sub.map(|s| s.as_str()) {
                    Some("status") => Some(FormatterKind::GitStatus),
                    Some("diff") => Some(FormatterKind::GitDiff),
                    Some("log") => Some(FormatterKind::GitLog),
                    _ => None,
                }
            }
            "cargo" => {
                let sub = rest.iter().find(|a| !a.starts_with('-'));
                match sub.map(|s| s.as_str()) {
                    Some("test") | Some("nextest") => Some(FormatterKind::CargoTest),
                    Some("check") => Some(FormatterKind::CargoCheck),
                    Some("clippy") => Some(FormatterKind::CargoClippy),
                    _ => None,
                }
            }
            "npm" | "pnpm" | "yarn" | "bun" => {
                let sub = rest.iter().find(|a| !a.starts_with('-')).map(|s| s.as_str()).unwrap_or("");
                if sub == "test" || rest.iter().any(|a| *a == "vitest" || *a == "jest") {
                    Some(FormatterKind::JestVitest)
                } else if sub == "run" && rest.iter().any(|a| *a == "check" || *a == "lint" || *a == "build") {
                    Some(FormatterKind::NpmYarn)
                } else {
                    Some(FormatterKind::NpmYarn)
                }
            }
            "vitest" | "jest" => Some(FormatterKind::JestVitest),
            "pytest" => Some(FormatterKind::Pytest),
            "ls" => Some(FormatterKind::Ls),
            "grep" | "rg" => Some(FormatterKind::Grep),
            "pip" | "pip3" => {
                let sub = rest.iter().find(|a| !a.starts_with('-'));
                if sub.map(|s| s.as_str()) == Some("list") {
                    Some(FormatterKind::PipList)
                } else {
                    None
                }
            }
            _ => None,
        };

        ClassificationMeta {
            original_command: cmd.to_string(),
            recognized_executable: Some(u.executable),
            wrapper_chain: u.wrapper_chain,
            selected_profile: profile,
            method: "deterministic_unwrapped",
            confidence: if profile.is_some() { 1.0 } else { 0.0 },
            passthrough_reason: if profile.is_none() { Some("unrecognized underlying executable") } else { None },
        }
    } else {
        ClassificationMeta {
            original_command: cmd.to_string(),
            recognized_executable: None,
            wrapper_chain: Vec::new(),
            selected_profile: None,
            method: "passthrough",
            confidence: 0.0,
            passthrough_reason: err_reason,
        }
    }
}

/// Classify which formatter owns this command using bounded recursive wrapper recognition.
pub fn classify(cmd: &str) -> Option<FormatterKind> {
    let raw_tokens = parse_command_tokens(cmd)?;

    let mut current_slice = raw_tokens.as_slice();
    let mut depth = 0;

    loop {
        match unwrap_step(current_slice) {
            Ok(Some((remaining, _wrapper))) => {
                if depth >= MAX_WRAPPER_DEPTH {
                    return None;
                }
                current_slice = remaining;
                depth += 1;
            }
            Ok(None) => break,
            Err(_) => return None,
        }
    }

    let mut idx = 0;
    while idx < current_slice.len() && is_env_assignment(&current_slice[idx]) {
        idx += 1;
    }

    if idx >= current_slice.len() {
        return None;
    }

    let exe = current_slice[idx].rsplit('/').next().unwrap_or(&current_slice[idx]);
    let rest = &current_slice[idx + 1..];

    match exe {
        "git" => {
            let sub = rest.iter().find(|a| !a.starts_with('-'))?;
            match sub.as_str() {
                "status" => Some(FormatterKind::GitStatus),
                "diff" => Some(FormatterKind::GitDiff),
                "log" => Some(FormatterKind::GitLog),
                _ => None,
            }
        }
        "cargo" => {
            let sub = rest.iter().find(|a| !a.starts_with('-'))?;
            match sub.as_str() {
                "test" | "nextest" => Some(FormatterKind::CargoTest),
                "check" => Some(FormatterKind::CargoCheck),
                "clippy" => Some(FormatterKind::CargoClippy),
                _ => None,
            }
        }
        "npm" | "pnpm" | "yarn" | "bun" => {
            let sub = rest.iter().find(|a| !a.starts_with('-')).map(|s| s.as_str()).unwrap_or("");
            if sub == "test" || rest.iter().any(|a| a == "vitest" || a == "jest") {
                Some(FormatterKind::JestVitest)
            } else if sub == "run" && rest.iter().any(|a| a == "check" || a == "lint" || a == "build") {
                Some(FormatterKind::NpmYarn)
            } else {
                Some(FormatterKind::NpmYarn)
            }
        }
        "vitest" | "jest" => Some(FormatterKind::JestVitest),
        "pytest" => Some(FormatterKind::Pytest),
        "ls" => Some(FormatterKind::Ls),
        "grep" | "rg" => Some(FormatterKind::Grep),
        "pip" | "pip3" => {
            let sub = rest.iter().find(|a| !a.starts_with('-'))?;
            if sub == "list" {
                Some(FormatterKind::PipList)
            } else {
                None
            }
        }
        "make" | "just" | "nx" | "turbo" => {
            // Task runner: check if repository architecture has a matching task profile
            check_project_task_profile(exe, rest)
        }
        _ => None,
    }
}

/// In-memory cache of task profiles loaded once per process to avoid disk I/O in the classification hot loop.
static TASK_PROFILES_CACHE: std::sync::OnceLock<std::collections::HashMap<String, FormatterKind>> =
    std::sync::OnceLock::new();

/// Query cached `.onyx/project/architecture.json` task profiles for task runners.
fn check_project_task_profile(runner: &str, rest: &[String]) -> Option<FormatterKind> {
    let target = rest.iter().find(|a| !a.starts_with('-')).map(|s| s.as_str()).unwrap_or("");
    if target.is_empty() {
        return None;
    }

    let cache = TASK_PROFILES_CACHE.get_or_init(|| {
        use std::fs::File;
        use std::io::BufReader;
        use std::path::PathBuf;

        let mut map = std::collections::HashMap::new();
        let workdir = std::env::var("ONYX_WORKDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        let arch_path = workdir.join(".onyx/project/architecture.json");
        if let Ok(file) = File::open(arch_path) {
            if let Ok(val) = serde_json::from_reader::<_, serde_json::Value>(BufReader::new(file)) {
                if let Some(task_profiles) = val.get("task_profiles").and_then(|v| v.as_object()) {
                    for (k, prof) in task_profiles {
                        let fmt_str = prof.get("formatter").and_then(|v| v.as_str()).unwrap_or("");
                        let fk = match fmt_str {
                            "cargo_test" => Some(FormatterKind::CargoTest),
                            "cargo_check" => Some(FormatterKind::CargoCheck),
                            "cargo_clippy" => Some(FormatterKind::CargoClippy),
                            "jest_vitest" => Some(FormatterKind::JestVitest),
                            "pytest" => Some(FormatterKind::Pytest),
                            "npm_yarn" => Some(FormatterKind::NpmYarn),
                            _ => None,
                        };
                        if let Some(f) = fk {
                            map.insert(k.clone(), f);
                        }
                    }
                }
            }
        }
        map
    });

    let query1 = format!("{runner} {target}");
    let query2 = format!("{runner} run {target}");

    if let Some(fmt) = cache.get(&query1).or_else(|| cache.get(&query2)) {
        return Some(*fmt);
    }

    // Default fallback heuristics for standard targets
    if runner == "nx" && (target == "test" || target.ends_with(":test")) {
        Some(FormatterKind::JestVitest)
    } else if runner == "turbo" && (target == "test" || target.ends_with(":test")) {
        Some(FormatterKind::JestVitest)
    } else {
        None
    }
}



// ============================================================================
// 1. CARGO CHECK & CARGO CLIPPY
// ============================================================================

fn cargo_diagnostics(
    out: &str,
    kind: FormatterKind,
    focus_files: Option<&[String]>,
) -> (CommandObservation, String) {
    let mut errors: Vec<DiagnosticEntry> = Vec::new();
    let mut warnings: Vec<DiagnosticEntry> = Vec::new();
    let mut current: Option<DiagnosticEntry> = None;
    let mut is_error = false;
    let mut snippet_lines: Vec<String> = Vec::new();

    for line in out.lines() {
        let t = line.trim_end();
        if t.starts_with("error[") || (t.starts_with("error:") && !t.contains("could not compile") && !t.contains("aborting")) {
            if let Some(mut prev) = current.take() {
                if !snippet_lines.is_empty() {
                    prev.snippet = Some(snippet_lines.join("\n"));
                    snippet_lines.clear();
                }
                if is_error {
                    errors.push(prev);
                } else {
                    warnings.push(prev);
                }
            }
            is_error = true;
            let code = if let Some(open) = t.find('[') {
                t[open + 1..].split(']').next().map(|s| s.to_string())
            } else {
                None
            };
            let msg = t.split(':').nth(1).unwrap_or(t).trim().to_string();
            current = Some(DiagnosticEntry {
                code,
                message: msg,
                ..Default::default()
            });
            continue;
        } else if t.starts_with("warning:") && !t.contains("generated") {
            if let Some(mut prev) = current.take() {
                if !snippet_lines.is_empty() {
                    prev.snippet = Some(snippet_lines.join("\n"));
                    snippet_lines.clear();
                }
                if is_error {
                    errors.push(prev);
                } else {
                    warnings.push(prev);
                }
            }
            is_error = false;
            let msg = t.strip_prefix("warning:").unwrap_or(t).trim().to_string();
            current = Some(DiagnosticEntry {
                message: msg,
                ..Default::default()
            });
            continue;
        }

        if let Some(curr) = current.as_mut() {
            if let Some(arrow) = t.find("--> ") {
                let loc = &t[arrow + 4..];
                let parts: Vec<&str> = loc.trim().split(':').collect();
                if parts.len() >= 2 {
                    curr.file = Some(parts[0].to_string());
                    curr.line = parts[1].parse().ok();
                    if parts.len() >= 3 {
                        curr.column = parts[2].parse().ok();
                    }
                }
            } else if t.starts_with("help:") || t.starts_with("suggestion:") {
                curr.suggestion = Some(t.to_string());
            } else if t.starts_with("   |") || t.starts_with("  |") || t.starts_with("    |") {
                if snippet_lines.len() < 8 {
                    snippet_lines.push(t.to_string());
                }
            }
        }
    }

    if let Some(mut prev) = current.take() {
        if !snippet_lines.is_empty() {
            prev.snippet = Some(snippet_lines.join("\n"));
        }
        if is_error {
            errors.push(prev);
        } else {
            warnings.push(prev);
        }
    }

    if errors.is_empty() && warnings.is_empty() {
        return (
            CommandObservation {
                kind: Some(kind),
                raw_preserved: true,
                ..Default::default()
            },
            out.to_string(),
        );
    }

    // Architecture-aware filtering: prioritize diagnostics matching focus_files
    let focus_set: Option<HashSet<&str>> = focus_files.map(|files| files.iter().map(|s| s.as_str()).collect());

    let mut result = String::new();
    let label = if kind == FormatterKind::CargoClippy { "clippy" } else { "check" };
    result.push_str(&format!(
        "cargo {label}: {} error(s), {} warning(s)\n",
        errors.len(),
        warnings.len()
    ));

    // Render errors (always included)
    for err in &errors {
        let is_focus = focus_set
            .as_ref()
            .map(|f| err.file.as_deref().map(|p| f.contains(p)).unwrap_or(false))
            .unwrap_or(false);
        let prefix = if is_focus { "🎯 [Task Focus] " } else { "" };
        let loc = err
            .file
            .as_deref()
            .map(|f| format!(" --> {f}:{}:{}", err.line.unwrap_or(1), err.column.unwrap_or(1)))
            .unwrap_or_default();
        let code = err.code.as_deref().map(|c| format!("[{c}] ")).unwrap_or_default();
        result.push_str(&format!("{prefix}error {code}{}:{}\n", err.message, loc));
        if let Some(snip) = &err.snippet {
            result.push_str(snip);
            result.push('\n');
        }
        if let Some(sug) = &err.suggestion {
            result.push_str(&format!("  {sug}\n"));
        }
    }

    // Render warnings: if too many, prioritize focus_files or cap at 3
    let mut rendered_warnings = 0;
    for warn in &warnings {
        let is_focus = focus_set
            .as_ref()
            .map(|f| warn.file.as_deref().map(|p| f.contains(p)).unwrap_or(false))
            .unwrap_or(false);
        if is_focus || rendered_warnings < 3 {
            let loc = warn
                .file
                .as_deref()
                .map(|f| format!(" --> {f}:{}", warn.line.unwrap_or(1)))
                .unwrap_or_default();
            result.push_str(&format!("warning: {}{loc}\n", warn.message));
            rendered_warnings += 1;
        }
    }
    if warnings.len() > rendered_warnings {
        result.push_str(&format!(
            "... (+{} additional compiler warning(s) omitted)\n",
            warnings.len() - rendered_warnings
        ));
    }

    let files_mentioned: Vec<String> = errors
        .iter()
        .chain(warnings.iter())
        .filter_map(|e| e.file.clone())
        .collect();

    let obs = CommandObservation {
        kind: Some(kind),
        summary_line: format!("cargo {label}: {} error(s), {} warning(s)", errors.len(), warnings.len()),
        errors,
        warnings,
        files_mentioned,
        ..Default::default()
    };

    (obs, result)
}

// ============================================================================
// 2. GIT DIFF
// ============================================================================

fn git_diff(out: &str, focus_files: Option<&[String]>) -> (CommandObservation, String) {
    let focus_set: Option<HashSet<&str>> = focus_files.map(|files| files.iter().map(|s| s.as_str()).collect());
    let mut files: Vec<String> = Vec::new();
    let mut result = String::new();
    let mut _current_file: Option<String> = None;
    let mut hunk_header = false;
    let mut lines_in_hunk = 0;
    let mut current_is_focus = false;

    for line in out.lines() {
        if line.starts_with("diff --git ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                let file_path = parts[3].trim_start_matches("b/").to_string();
                current_is_focus = focus_set
                    .as_ref()
                    .map(|f| f.contains(file_path.as_str()))
                    .unwrap_or(true);
                files.push(file_path.clone());
                _current_file = Some(file_path.clone());
                result.push_str(&format!("\n--- a/{file_path} +++ b/{file_path}\n"));
            }
            hunk_header = false;
            lines_in_hunk = 0;
            continue;
        }

        if line.starts_with("@@") {
            hunk_header = true;
            lines_in_hunk = 0;
            result.push_str(line);
            result.push('\n');
            continue;
        }

        if hunk_header {
            if line.starts_with('+') || line.starts_with('-') {
                result.push_str(line);
                result.push('\n');
                lines_in_hunk += 1;
            } else if lines_in_hunk < 3 && current_is_focus {
                // Keep minimal surrounding context
                result.push_str(line);
                result.push('\n');
                lines_in_hunk += 1;
            }
        }
    }

    if files.is_empty() {
        return (
            CommandObservation {
                kind: Some(FormatterKind::GitDiff),
                raw_preserved: true,
                ..Default::default()
            },
            out.to_string(),
        );
    }

    let summary = format!("git diff: {} file(s) modified", files.len());
    let obs = CommandObservation {
        kind: Some(FormatterKind::GitDiff),
        summary_line: summary,
        files_mentioned: files,
        ..Default::default()
    };

    (obs, result.trim().to_string())
}

// ============================================================================
// 3. GIT LOG
// ============================================================================

fn git_log(out: &str) -> (CommandObservation, String) {
    let mut commits = Vec::new();
    let mut current_hash = String::new();
    let mut current_msg = String::new();
    let mut current_date = String::new();

    for line in out.lines() {
        let t = line.trim();
        if t.starts_with("commit ") {
            if !current_hash.is_empty() {
                commits.push(format!("{} · {} ({})", &current_hash[..7.min(current_hash.len())], current_msg, current_date));
            }
            current_hash = t.strip_prefix("commit ").unwrap_or("").trim().to_string();
            current_msg.clear();
            current_date.clear();
        } else if t.starts_with("Date:") {
            current_date = t.strip_prefix("Date:").unwrap_or("").trim().to_string();
        } else if !t.starts_with("Author:") && !t.is_empty() && current_msg.is_empty() {
            current_msg = t.to_string();
        }
    }
    if !current_hash.is_empty() {
        commits.push(format!("{} · {} ({})", &current_hash[..7.min(current_hash.len())], current_msg, current_date));
    }

    if commits.is_empty() {
        return (
            CommandObservation {
                kind: Some(FormatterKind::GitLog),
                raw_preserved: true,
                ..Default::default()
            },
            out.to_string(),
        );
    }

    let count = commits.len();
    let mut result = format!("git log: {count} commit(s)\n");
    for c in commits.iter().take(15) {
        result.push_str(&format!("  {c}\n"));
    }
    if count > 15 {
        result.push_str(&format!("  ... (+{} older commits omitted)\n", count - 15));
    }

    let obs = CommandObservation {
        kind: Some(FormatterKind::GitLog),
        summary_line: format!("git log: {count} commit(s)"),
        ..Default::default()
    };

    (obs, result)
}

// ============================================================================
// 4. JEST / VITEST
// ============================================================================

fn jest_vitest(out: &str, focus_files: Option<&[String]>) -> (CommandObservation, String) {
    let mut failed_tests: Vec<String> = Vec::new();
    let mut failed_files: Vec<String> = Vec::new();
    let mut failure_blocks: Vec<String> = Vec::new();
    let mut current_block: Vec<String> = Vec::new();
    let mut in_fail = false;
    let mut passed = 0;
    let mut failed = 0;

    for line in out.lines() {
        let t = line.trim_end();
        if t.contains("FAIL ") || t.contains("✕ ") || t.contains("✖ ") {
            in_fail = true;
            if t.contains("FAIL ") {
                let p = t.split("FAIL ").nth(1).unwrap_or("").trim().to_string();
                if !failed_files.contains(&p) {
                    failed_files.push(p);
                }
            } else {
                failed_tests.push(t.to_string());
            }
            current_block.push(t.to_string());
            continue;
        }

        if in_fail {
            if t.contains("● ") || t.contains("AssertionError") || t.contains("Error:") || t.starts_with("      at ") {
                current_block.push(t.to_string());
            } else if t.is_empty() || t.contains("PASS ") || t.starts_with("Test Suites:") {
                if !current_block.is_empty() {
                    failure_blocks.push(current_block.join("\n"));
                    current_block.clear();
                }
                in_fail = false;
            }
        }

        if t.contains("Tests:") {
            if let Some(p) = grab_number(t, "passed") {
                passed = p;
            }
            if let Some(f) = grab_number(t, "failed") {
                failed = f;
            }
        }
    }

    if !current_block.is_empty() {
        failure_blocks.push(current_block.join("\n"));
    }

    if failed_tests.is_empty() && failure_blocks.is_empty() && passed == 0 {
        return (
            CommandObservation {
                kind: Some(FormatterKind::JestVitest),
                raw_preserved: true,
                ..Default::default()
            },
            out.to_string(),
        );
    }

    let mut result = format!("test runner (jest/vitest): {passed} passed, {failed} failed\n");
    let mut ordered_files = failed_files.clone();
    if let Some(focus) = focus_files {
        ordered_files.sort_by_key(|file| if is_focus_path(file, focus) { 0 } else { 1 });
    }
    for f in &ordered_files {
        let marker = if focus_files.map(|focus| is_focus_path(f, focus)).unwrap_or(false) {
            "[Task Focus] "
        } else {
            ""
        };
        result.push_str(&format!("FAIL: {marker}{f}\n"));
    }
    for block in &failure_blocks {
        result.push_str(block);
        result.push('\n');
    }

    let obs = CommandObservation {
        kind: Some(FormatterKind::JestVitest),
        summary_line: format!("{passed} passed, {failed} failed"),
        failed_tests,
        passed_tests_count: passed,
        failed_tests_count: failed,
        files_mentioned: failed_files,
        ..Default::default()
    };

    (obs, result)
}

// ============================================================================
// 5. PYTEST
// ============================================================================

fn pytest(out: &str, focus_files: Option<&[String]>) -> (CommandObservation, String) {
    let mut failed_tests: Vec<String> = Vec::new();
    let mut failure_blocks: Vec<String> = Vec::new();
    let mut current_block: Vec<String> = Vec::new();
    let mut in_fail = false;
    let mut summary = String::new();

    for line in out.lines() {
        let t = line.trim_end();
        if t.starts_with("FAILED ") {
            let name = t.strip_prefix("FAILED ").unwrap_or("").to_string();
            failed_tests.push(name);
            continue;
        }

        if t.starts_with("=== FAILURES ===") || t.starts_with("___ ") {
            in_fail = true;
            if !current_block.is_empty() {
                failure_blocks.push(current_block.join("\n"));
                current_block.clear();
            }
            current_block.push(t.to_string());
            continue;
        }

        if in_fail {
            if t.starts_with("=== short test summary info ===") || t.starts_with("=== ") && t.contains("passed") {
                if !current_block.is_empty() {
                    failure_blocks.push(current_block.join("\n"));
                    current_block.clear();
                }
                in_fail = false;
            } else if t.contains("assert ") || t.contains("E   ") || t.starts_with("    at ") || t.contains("Error:") {
                current_block.push(t.to_string());
            }
        }

        if t.starts_with("===") && (t.contains("passed") || t.contains("failed")) {
            summary = t.replace('=', "").trim().to_string();
        }
    }

    if !current_block.is_empty() {
        failure_blocks.push(current_block.join("\n"));
    }

    if failed_tests.is_empty() && summary.is_empty() {
        return (
            CommandObservation {
                kind: Some(FormatterKind::Pytest),
                raw_preserved: true,
                ..Default::default()
            },
            out.to_string(),
        );
    }

    let mut result = format!("pytest: {}\n", if summary.is_empty() { "completed" } else { &summary });
    let mut ordered_tests = failed_tests.clone();
    if let Some(focus) = focus_files {
        ordered_tests.sort_by_key(|test| if is_focus_path(test, focus) { 0 } else { 1 });
    }
    for fail in &ordered_tests {
        let marker = if focus_files.map(|focus| is_focus_path(fail, focus)).unwrap_or(false) {
            "[Task Focus] "
        } else {
            ""
        };
        result.push_str(&format!("FAILED: {marker}{fail}\n"));
    }
    for b in &failure_blocks {
        result.push_str(b);
        result.push('\n');
    }

    let obs = CommandObservation {
        kind: Some(FormatterKind::Pytest),
        summary_line: summary,
        failed_tests_count: failed_tests.len(),
        failed_tests,
        ..Default::default()
    };

    (obs, result)
}

// ============================================================================
// 6. NPM / PNPM / YARN PACKAGE MANAGERS
// ============================================================================

fn npm_yarn(out: &str) -> (CommandObservation, String) {
    let mut errors: Vec<String> = Vec::new();
    let mut warnings_count = 0;
    let mut summary = String::new();

    for line in out.lines() {
        let t = line.trim_end();
        if t.contains("ERR!") || t.contains("error ") || t.contains("Error:") {
            errors.push(t.to_string());
        } else if t.contains("WARN") || t.contains("warning ") {
            warnings_count += 1;
        } else if t.contains("packages are looking for funding") || t.contains("found ") && t.contains("vulnerabilities") {
            summary = t.to_string();
        }
    }

    if errors.is_empty() && summary.is_empty() {
        return (
            CommandObservation {
                kind: Some(FormatterKind::NpmYarn),
                raw_preserved: true,
                ..Default::default()
            },
            out.to_string(),
        );
    }

    let mut result = String::new();
    if !errors.is_empty() {
        result.push_str(&format!("npm/yarn error: {} error(s), {} warning(s)\n", errors.len(), warnings_count));
        for e in errors.iter().take(10) {
            result.push_str(&format!("  {e}\n"));
        }
    } else {
        result.push_str(&format!("npm/yarn success: {summary}\n"));
    }

    let obs = CommandObservation {
        kind: Some(FormatterKind::NpmYarn),
        summary_line: format!("{} error(s), {} warning(s)", errors.len(), warnings_count),
        ..Default::default()
    };

    (obs, result)
}

// ============================================================================
// 7. CARGO TEST (Enhanced with Structured Observation)
// ============================================================================

fn cargo_test(out: &str) -> (CommandObservation, String) {
    let mut failed_lines: Vec<String> = Vec::new();
    let mut error_blocks: Vec<String> = Vec::new();
    let mut passed = 0usize;
    let mut filtered_out = 0usize;
    let mut ignored = 0usize;
    let mut suites = 0usize;
    let mut saw_result = false;
    let mut in_error_block = false;
    let mut block = String::new();

    for line in out.lines() {
        let t = line.trim_end();

        if let Some(res) = t.strip_prefix("test result:") {
            saw_result = true;
            suites += 1;
            passed += grab_number(res, "passed").unwrap_or(0);
            filtered_out += grab_number(res, "filtered out").unwrap_or(0);
            ignored += grab_number(res, "ignored").unwrap_or(0);
            let fails = grab_number(res, "failed").unwrap_or(0);
            if fails > 0 {
                failed_lines.push(format!("[{fails} failed] {t}"));
            }
            continue;
        }
        if t.starts_with("test ") && t.ends_with("FAILED") {
            failed_lines.push(t.to_string());
            continue;
        }
        if t.starts_with("error[E") || (t.starts_with("error:") && !t.contains("test result")) {
            in_error_block = true;
            block.clear();
            block.push_str(t);
            block.push('\n');
            continue;
        }
        if in_error_block {
            if t.is_empty() || t.starts_with("-->") || t.starts_with("at ") || looks_like_continuation(t) {
                block.push_str(t);
                block.push('\n');
            } else {
                error_blocks.push(block.trim_end().to_string());
                in_error_block = false;
                block.clear();
            }
        }
    }
    if in_error_block && !block.trim().is_empty() {
        error_blocks.push(block.trim_end().to_string());
    }

    if !saw_result && error_blocks.is_empty() && failed_lines.is_empty() {
        return (
            CommandObservation {
                kind: Some(FormatterKind::CargoTest),
                raw_preserved: true,
                ..Default::default()
            },
            out.to_string(),
        );
    }

    let mut result = String::new();
    for b in &error_blocks {
        result.push_str(b);
        result.push('\n');
    }
    for f in &failed_lines {
        result.push_str(f);
        result.push('\n');
    }
    if saw_result {
        result.push_str(&format!(
            "tests: {suites} suite(s) · {passed} passed · {ignored} ignored · {filtered_out} filtered out\n"
        ));
    }

    let obs = CommandObservation {
        kind: Some(FormatterKind::CargoTest),
        summary_line: format!("{suites} suite(s) · {passed} passed · {} failed", failed_lines.len()),
        failed_tests_count: failed_lines.len(),
        passed_tests_count: passed,
        failed_tests: failed_lines,
        ..Default::default()
    };

    (obs, result)
}

// ============================================================================
// 8. GIT STATUS
// ============================================================================

fn git_status(out: &str) -> String {
    let mut group_counts: Vec<(String, usize)> = Vec::new();
    let mut current: Option<String> = None;
    let mut branch: Option<String> = None;
    let mut ahead_behind: Option<String> = None;
    let mut untracked = 0usize;
    let mut unknown_body = false;

    for line in out.lines() {
        let t = line.trim_end();
        if t.is_empty() {
            continue;
        }
        if let Some(b) = t.strip_prefix("On branch ") {
            branch = Some(b.to_string());
            continue;
        }
        if let Some(rest) = t.strip_prefix("Your branch is ahead of ") {
            ahead_behind = extract_commit_count(rest).map(|n| format!("ahead:{n}"));
            continue;
        } else if let Some(rest) = t.strip_prefix("Your branch is behind ") {
            ahead_behind = extract_commit_count(rest).map(|n| format!("behind:{n}"));
            continue;
        } else if t.starts_with("Your branch is") {
            continue;
        }

        let header = if t.ends_with(':') && !t.starts_with('\t') {
            Some(t.trim_end_matches(':').to_string())
        } else {
            None
        };

        if let Some(h) = header {
            let l = h.to_lowercase();
            let is_untracked = l.contains("untracked");
            let known = l.contains("staged")
                || is_untracked
                || l.contains("deleted")
                || l.contains("renamed")
                || l.contains("changed")
                || l.contains("both modified")
                || l.contains("fixed up")
                || l.contains("copied")
                || l.contains("merged")
                || l.contains("to be committed");
            if !known {
                unknown_body = true;
            }
            current = Some(if is_untracked { "UNTRACKED".to_string() } else { h.clone() });
            if !is_untracked && !group_counts.iter().any(|(n, _)| *n == h) {
                group_counts.push((h, 0));
            }
            continue;
        }

        if let Some(c) = &current {
            if t.starts_with('\t') {
                if c == "UNTRACKED" {
                    untracked += 1;
                } else if let Some(slot) = group_counts.iter_mut().find(|(n, _)| n == c) {
                    slot.1 += 1;
                }
            } else if !t.starts_with("  (") {
                unknown_body = true;
            }
        } else if t.starts_with('\t') {
            unknown_body = true;
        }
    }

    if unknown_body {
        return out.to_string();
    }

    let any_content = group_counts.iter().any(|(_, c)| *c > 0) || untracked > 0;
    if !any_content {
        return out.to_string();
    }

    let mut result = String::new();
    if let Some(b) = branch {
        result.push_str(&format!("branch {b}"));
        if let Some(ab) = ahead_behind {
            result.push_str(&format!(" ({ab})"));
        }
        result.push('\n');
    }
    for (name, count) in &group_counts {
        if *count == 0 {
            continue;
        }
        result.push_str(&format!("{name}: {count}\n"));
    }
    if untracked > 0 {
        result.push_str(&format!("untracked: {untracked}\n"));
    }
    result
}

fn extract_commit_count(rest: &str) -> Option<usize> {
    let after = rest.split(" by ").nth(1)?;
    after.split_whitespace().next()?.parse().ok()
}

// ============================================================================
// 9. LS & GREP & PIP LIST
// ============================================================================

fn ls(out: &str) -> String {
    let mut files: Vec<String> = Vec::new();
    let mut dirs: Vec<String> = Vec::new();
    let mut saw_long_format = false;

    for line in out.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("total ") {
            continue;
        }
        let cols: Vec<&str> = t.split_whitespace().collect();
        if cols.len() >= 9 && (cols[0].starts_with('d') || cols[0].starts_with('-') || cols[0].starts_with('l')) {
            saw_long_format = true;
            let name = cols[8..].join(" ");
            if cols[0].starts_with('d') {
                dirs.push(name);
            } else {
                files.push(name);
            }
        }
    }

    if !saw_long_format {
        return out.to_string();
    }

    let mut result = format!("{} file(s), {} dir(s):\n", files.len(), dirs.len());
    for d in dirs.iter().take(20) {
        result.push_str(&format!("  {d}/\n"));
    }
    for f in files.iter().take(30) {
        result.push_str(&format!("  {f}\n"));
    }
    if files.len() > 30 || dirs.len() > 20 {
        result.push_str("  ...\n");
    }
    result
}

fn grep(out: &str) -> String {
    let mut by_file: Vec<(String, Vec<String>)> = Vec::new();
    for line in out.lines() {
        if let Some((f, rest)) = line.split_once(':') {
            if let Some(entry) = by_file.iter_mut().find(|(path, _)| path == f) {
                entry.1.push(rest.to_string());
            } else {
                by_file.push((f.to_string(), vec![rest.to_string()]));
            }
        }
    }

    if by_file.is_empty() {
        return out.to_string();
    }

    let mut result = String::new();
    for (file, matches) in by_file {
        result.push_str(&format!("{file} ({} matches):\n", matches.len()));
        for m in matches.iter().take(5) {
            result.push_str(&format!("  {m}\n"));
        }
        if matches.len() > 5 {
            result.push_str(&format!("  ... (+{} more match(es))\n", matches.len() - 5));
        }
    }
    result
}

fn grouped_lines(out: &str, tag: &str) -> String {
    let mut rows: Vec<String> = Vec::new();
    for line in out.lines() {
        let t = line.trim_end();
        if t.is_empty() || t.starts_with('-') || t.to_lowercase().starts_with(tag) {
            continue;
        }
        rows.push(t.to_string());
    }
    if rows.len() < 12 {
        return out.to_string();
    }
    let mut fams: Vec<(String, usize)> = Vec::new();
    for r in &rows {
        let word = r.split_whitespace().next().unwrap_or("");
        let stem = word.split('-').next().unwrap_or(word).to_string();
        if let Some(slot) = fams.iter_mut().find(|(s, _)| *s == stem) {
            slot.1 += 1;
        } else {
            fams.push((stem, 1));
        }
    }
    let mut result = format!("{} package(s):\n", rows.len());
    fams.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    for (stem, count) in fams {
        result.push_str(&format!("{stem}* : {count}\n"));
    }
    result
}

fn grab_number(hay: &str, label: &str) -> Option<usize> {
    let idx = hay.find(label)?;
    let head = hay[..idx].trim_end();
    head.split(|c: char| !c.is_ascii_digit())
        .next_back()
        .and_then(|d| d.parse().ok())
}

fn is_focus_path(value: &str, focus_files: &[String]) -> bool {
    focus_files
        .iter()
        .any(|focus| value.contains(focus) || focus.contains(value))
}

fn looks_like_continuation(t: &str) -> bool {
    t.starts_with("   |")
        || t.starts_with("  |")
        || t.starts_with("note:")
        || t.starts_with("help:")
        || t.starts_with("=")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_output_is_left_alone() {
        let (o, s) = compact("git status", "short");
        assert_eq!(o, "short");
        assert_eq!(s, Savings::Small(5));
    }

    #[test]
    fn git_status_groups_into_counts() {
        let pad = "crates/agent/src/retrieval/tests/fixtures/generated_case";
        let raw = format!(
            "On branch main\nYour branch is ahead of 'origin/main' by 2 commits.\n  (use \"git push\" to publish your local commits)\n\nChanges to be committed:\n  (use \"git restore --staged <file>...\" to unstage)\n\tmodified:   {pad}a.rs\n\tmodified:   {pad}b.rs\n\tnew file:   {pad}c.rs\n\nChanges not staged for commit:\n  (use \"git add <file>...\" to update content to be committed)\n\tmodified:   {pad}d.rs\n\tdeleted:    {pad}e.rs\n\nUntracked files:\n  (use \"git add <file>...\" to include in what will be committed)\n\t{pad}f.rs\n\t{pad}g.rs\n"
        );
        let (o, s) = compact("git status", &raw);
        assert!(matches!(s, Savings::Compacted { .. }), "{s:?}");
        assert!(o.contains("branch main"), "{o}");
        assert!(o.contains("ahead:"), "{o}");
        assert!(o.contains("Changes to be committed: 3"), "{o}");
        assert!(o.contains("not staged for commit: 2"), "{o}");
        assert!(o.contains("untracked: 2"), "{o}");
        assert!(o.len() < raw.len());
    }

    #[test]
    fn cargo_test_aggregates_results() {
        let raw = format!(
            "running 3 tests\ntest alpha::one ... ok\ntest beta::two ... ok\ntest gamma::three ... FAILED\nfailures:\n\n---- gamma::three stdout ----\npanic\n\n{}\ntest result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s",
            "padding ".repeat(60)
        );
        let (o, s) = compact("cargo test", &raw);
        assert!(matches!(s, Savings::Compacted { .. }), "{s:?}");
        assert!(o.contains("suite(s)"), "{o}");
        assert!(o.contains("2 passed"), "{o}");
        assert!(o.contains("gamma::three"), "{o}");
    }

    #[test]
    fn cargo_check_and_clippy_formats_diagnostics() {
        let raw = format!(
            "   Compiling my-crate v0.1.0 (/root/my-crate)\nerror[E0308]: mismatched types\n  --> src/main.rs:42:15\n   |\n42 |     let x: u32 = \"str\";\n   |            ---   ^^^^^ expected `u32`, found `&str`\n\nwarning: unused variable `y`\n  --> src/lib.rs:10:9\n   |\n10 |     let y = 100;\n   |\n{}\nerror: could not compile `my-crate` due to 1 previous error",
            "padding ".repeat(40)
        );
        let (o, s) = compact("cargo check", &raw);
        assert!(matches!(s, Savings::Compacted { .. }), "{s:?}");
        assert!(o.contains("cargo check: 1 error(s), 1 warning(s)"), "{o}");
        assert!(o.contains("error [E0308] mismatched types"), "{o}");
        assert!(o.contains("src/main.rs:42:15"), "{o}");
    }

    #[test]
    fn git_diff_collapses_unchanged_lines() {
        let raw = format!(
            "diff --git a/src/main.rs b/src/main.rs\nindex 1234..5678 100644\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -10,6 +10,7 @@\n{}\n-old_code();\n+new_code();\n{}\n",
            " unchanged_context();\n".repeat(25),
            " unchanged_footer();\n".repeat(25)
        );
        let (o, s) = compact("git diff", &raw);
        assert!(matches!(s, Savings::Compacted { .. }), "{s:?}");
        assert!(o.contains("+new_code();"), "{o}");
        assert!(o.contains("-old_code();"), "{o}");
        assert!(o.len() < raw.len());
    }

    #[test]
    fn jest_vitest_extracts_failures() {
        let raw = format!(
            "FAIL src/app.test.tsx\n  ✕ renders home page (45 ms)\n  ● renders home page\n    AssertionError: expected false to be true\n      at src/app.test.tsx:24:10\n\n{}\nTests: 1 failed, 14 passed, 15 total",
            "pass line\n".repeat(40)
        );
        let (o, s) = compact("npm test", &raw);
        assert!(matches!(s, Savings::Compacted { .. }), "{s:?}");
        assert!(o.contains("14 passed, 1 failed"), "{o}");
        assert!(o.contains("FAIL: src/app.test.tsx"), "{o}");
        assert!(o.contains("AssertionError"), "{o}");
    }

    #[test]
    fn pytest_extracts_failed_tests() {
        let raw = format!(
            "=== test session starts ===\ncollecting ...\n{}\nFAILED tests/test_api.py::test_login - assert 401 == 200\n=== FAILURES ===\n___ test_login ___\n    assert 401 == 200\nE   AssertionError\n=== 1 failed, 20 passed in 0.45s ===",
            "tests/test_x.py ....\n".repeat(30)
        );
        let (o, s) = compact("pytest", &raw);
        assert!(matches!(s, Savings::Compacted { .. }), "{s:?}");
        assert!(o.contains("FAILED: tests/test_api.py::test_login"), "{o}");
        assert!(o.contains("1 failed, 20 passed"), "{o}");
    }

    #[test]
    fn ls_collapses_long_format() {
        let raw = format!(
            "total 128\ndrwxr-xr-x 4 root root 4096 Jan 1 00:00 src\n-rw-r--r-- 1 root root 1234 Jan 1 00:00 Cargo.toml\n-rw-r--r-- 1 root root 5678 Jan 1 00:00 README.md\n{}",
            (0..8).map(|i| format!("-rw-r--r-- 1 root root {} Jan 1 00:00 filler_file_{i}.rs", 100 + i)).collect::<Vec<_>>().join("\n")
        );
        let (o, s) = compact("ls -la", &raw);
        assert!(matches!(s, Savings::Compacted { .. }), "{s:?}");
        assert!(o.contains("src/"), "{o}");
        assert!(o.contains("10 file(s), 1 dir(s)"), "{o}");
    }

    #[test]
    fn grep_caps_and_dedups_per_file() {
        let mut raw = String::new();
        for i in 0..30 {
            raw.push_str(&format!("some/long/path/hot.rs:{i}: matched token number {i}\n"));
        }
        let (o, s) = compact("grep -rn token .", &raw);
        assert!(matches!(s, Savings::Compacted { .. }), "{s:?}");
        assert!(o.contains("more match(es)"), "{o}");
        assert!(o.len() < raw.len());
    }

    #[test]
    fn unknown_command_passthrough() {
        let raw = "whatever ".repeat(100);
        let (o, s) = compact("myapp --dump", &raw);
        assert_eq!(o, raw);
        assert_eq!(s, Savings::Unchanged(raw.len()));
    }

    #[test]
    fn never_grows_output() {
        let raw = "On branch x\n".to_string() + &"y".repeat(600);
        let (o, _) = compact("git status", &raw);
        assert!(o.len() <= raw.len());
    }

    #[test]
    fn telemetry_gain_report_accuracy() {
        reset_telemetry();
        let raw = format!(
            "   Compiling test v0.1\nerror[E0425]: cannot find value `foo`\n  --> src/lib.rs:5:10\n{}\nerror: aborting",
            "filler\n".repeat(50)
        );
        let (_, s) = compact("cargo check", &raw);
        record(s);
        let report = gain_report();
        assert!(report.bytes_saved > 0);
        assert!(report.reduction_pct > 0.0);
        assert!(report.calls == 1);
        assert_eq!(report.estimated_tokens_saved, report.bytes_saved / 4);
    }
}
