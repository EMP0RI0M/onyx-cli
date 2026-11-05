//! Synchronous facade over the async sleep cycle for Onyx's TUI thread.
//!
//! The interactive UI owns one OS thread and must not block on an executor it
//! does not control, and it should not have to depend on tokio just to reach this
//! engine. So the runtime lives here, beside the code that needs it: callers hand
//! in plain data and get a plain result back.

use crate::skillopt_sleep::onyx_cycle::{run_sleep_cycle, SleepCycleInput, SleepCycleOutcome};

/// Errors surfaced by a sleep cycle, already flattened to text for the UI.
#[derive(Debug)]
pub struct SleepError(pub String);

impl std::fmt::Display for SleepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for SleepError {}

/// Run one offline consolidation cycle to completion on the calling thread.
///
/// `backend_name` is informational only — passing `Some` labels the run as
/// model-assisted but does not construct a client, because doing so would require
/// network credentials the CLI deliberately keeps out of this layer. The engine
/// runs its deterministic path either way.
pub fn run_cycle_blocking(input: SleepCycleInput) -> Result<SleepCycleOutcome, SleepError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| SleepError(format!("runtime: {e}")))?;

    runtime
        .block_on(run_sleep_cycle(input, None))
        .map_err(|e| SleepError(e.to_string()))
}

/// Convenience wrapper: harvest `sessions_dir`, optimize the given skills, return
/// the outcome. Kept separate from `SleepCycleInput`'s defaults so callers cannot
/// accidentally point the harvester at a relative `.onyx/sessions`.
pub fn optimize_skills(
    sessions_dir: std::path::PathBuf,
    skills: std::collections::HashMap<String, String>,
) -> Result<SleepCycleOutcome, SleepError> {
    run_cycle_blocking(SleepCycleInput {
        sessions_dir,
        skills,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocking_facade_drives_the_async_engine() {
        let dir = std::env::temp_dir().join(format!("onyx-facade-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("1-x.jsonl"),
            concat!(
                r#"{"kind":"meta","model":"m","ts":1,"cwd":"/r"}"#,
                "\n",
                r#"{"kind":"message","role":"user","content":"make the release job reproducible","ts":2}"#,
                "\n",
                r#"{"kind":"message","role":"assistant","content":"still broken, it failed again","ts":3}"#,
                "\n",
            ),
        )
        .unwrap();

        let mut skills = std::collections::HashMap::new();
        skills.insert("release".to_string(), "Tag and push.".to_string());
        let outcome = optimize_skills(dir.clone(), skills).unwrap();
        assert_eq!(outcome.digests.len(), 1);
        assert!(!outcome.reports.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_directory_is_not_an_error() {
        let outcome = optimize_skills(
            std::path::PathBuf::from("/nonexistent/onyx-sessions-test"),
            Default::default(),
        )
        .unwrap();
        assert!(outcome.digests.is_empty());
    }
}
