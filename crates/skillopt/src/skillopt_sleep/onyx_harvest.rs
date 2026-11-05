//! SkillOpt-Sleep — Stage 1 for Onyx: read `$ONYX_HOME/sessions/*.jsonl`.
//!
//! The vendored `harvest.rs` understands Claude Code transcripts (`type` +
//! nested `message`). Onyx writes its own append-only shape: a `{kind:"meta"}`
//! header followed by `{kind:"message", role, content, ts, tool_call?}`
//! records. This adapter normalises that shape into the same `SessionDigest`
//! the rest of the pipeline consumes, so mining, replay and consolidation stay
//! format-agnostic.

use crate::skillopt_sleep::harvest::detect_feedback;
use crate::skillopt_sleep::types::SessionDigest;
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Take the last non-empty line of a message body — that is the turn's actual
/// conclusion, whereas earlier lines are narration before the next tool call.
fn final_line(text: &str) -> Option<String> {
    text.lines().rev().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("<<<")).map(|l| l.to_string())
}

fn stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Parse one Onyx transcript into a normalized digest.
pub fn harvest_onyx_session_file<P: AsRef<Path>>(path: P) -> anyhow::Result<SessionDigest> {
    let path = path.as_ref();
    let file = File::open(path)?;
    let reader = BufReader::new(file);

    let mut digest = SessionDigest {
        session_id: stem(path),
        raw_path: path.to_string_lossy().into_owned(),
        ..Default::default()
    };

    // A user turn may be followed by assistant/tool records before the next
    // prompt; feedback phrases belong to the prompt that expresses them.
    let mut pending_user: Option<String> = None;

    for line in reader.lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(line) else { continue };

        match record.get("kind").and_then(Value::as_str).unwrap_or("") {
            "meta" => {
                if let Some(cwd) = record.get("cwd").and_then(Value::as_str) {
                    digest.project = cwd.to_string();
                }
                if let Some(b) = record.get("branch").and_then(Value::as_str) {
                    digest.git_branch = b.to_string();
                }
                if let Some(ts) = record.get("ts").and_then(Value::as_u64) {
                    digest.started_at = ts.to_string();
                }
                if let Some(t) = record.get("title").and_then(Value::as_str) {
                    digest.skills_used.push(format!("title:{t}"));
                }
            }
            "message" => {
                let role = record.get("role").and_then(Value::as_str).unwrap_or("");
                let content = record.get("content").and_then(Value::as_str).unwrap_or("");
                if let Some(ts) = record.get("ts").and_then(Value::as_u64) {
                    digest.ended_at = ts.to_string();
                }
                match role {
                    "user" => {
                        digest.n_user_turns += 1;
                        let text = content.trim();
                        if !text.is_empty() {
                            digest.feedback_signals.extend(detect_feedback(text));
                            digest.user_prompts.push(text.to_string());
                            pending_user = Some(text.to_string());
                        }
                    }
                    "assistant" => {
                        digest.n_assistant_turns += 1;
                        if let Some(tc) = record.get("tool_call") {
                            if let Some(name) = tc.get("name").and_then(Value::as_str) {
                                digest.tools_used.push(name.to_string());
                            }
                        }
                        if let Some(last) = final_line(content) {
                            digest.assistant_finals.push(last);
                        }
                    }
                    "tool" => {
                        // Onyx stores tool results as their own role; the name
                        // lives on the originating assistant record, so capture
                        // any file paths the output mentions.
                        for word in content.split_whitespace() {
                            if word.len() > 5 && word.contains('/') && !word.contains(' ') {
                                digest.files_touched.push(word.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    if let Some(last_user) = pending_user {
        // A trailing slash command is an instruction to the harness, not a task
        // the optimizer should learn from.
        if last_user.starts_with('/') {
            digest.user_prompts.retain(|p| *p != last_user);
        }
    }

    digest.tools_used.sort();
    digest.tools_used.dedup();
    digest.feedback_signals.sort();
    digest.feedback_signals.dedup();
    digest.files_touched.sort();
    digest.files_touched.dedup();
    digest.skills_used.retain(|s| !s.starts_with("title:"));

    Ok(digest)
}

/// Harvest every Onyx transcript under `dir`, newest-normalised first by id.
/// Unreadable files are skipped rather than aborting the sweep.
pub fn harvest_onyx_sessions_dir(dir: &Path, limit: usize) -> Vec<SessionDigest> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "jsonl").unwrap_or(false))
        .collect();
    // Transcript ids are epoch-based, so lexicographic order is chronological.
    paths.sort();
    paths.reverse();

    let mut out = Vec::new();
    for p in paths {
        if out.len() >= limit {
            break;
        }
        if let Ok(d) = harvest_onyx_session_file(&p) {
            if d.n_user_turns > 0 {
                out.push(d);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_onyx_transcript_shape() {
        let dir = std::env::temp_dir().join(format!("onyx-harvest-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("1700000000-abc.jsonl");
        {
            let mut f = File::create(&path).unwrap();
            writeln!(f, r#"{{"kind":"meta","model":"m","ts":1700000000,"cwd":"/work/proj","branch":"main"}}"#).unwrap();
            writeln!(f, r#"{{"kind":"message","role":"user","content":"Fix the failing parser in crates/kernel","ts":1700000010}}"#).unwrap();
            writeln!(f, r#"{{"kind":"message","role":"assistant","content":"looking at it\nnow","tool_call":{{"name":"bash","args":null}},"ts":1700000020}}"#).unwrap();
            writeln!(f, r#"{{"kind":"message","role":"tool","content":"read /work/proj/src/lib.rs ok","ts":1700000030}}"#).unwrap();
            writeln!(f, r#"{{"kind":"message","role":"assistant","content":"Fixed: the tokenizer was off by one.","ts":1700000040}}"#).unwrap();
            writeln!(f, r#"{{"kind":"message","role":"user","content":"thanks, works now","ts":1700000050}}"#).unwrap();
        }

        let d = harvest_onyx_session_file(&path).unwrap();
        assert_eq!(d.session_id, "1700000000-abc");
        assert_eq!(d.project, "/work/proj");
        assert_eq!(d.git_branch, "main");
        assert_eq!(d.n_user_turns, 2);
        assert_eq!(d.tools_used, vec!["bash".to_string()]);
        assert_eq!(d.assistant_finals.last().unwrap(), "Fixed: the tokenizer was off by one.");
        assert!(d.feedback_signals.iter().any(|s| s.starts_with("pos:")));
        assert!(d.files_touched.iter().any(|f| f.ends_with("lib.rs")));

        let swept = harvest_onyx_sessions_dir(&dir, 10);
        assert_eq!(swept.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
