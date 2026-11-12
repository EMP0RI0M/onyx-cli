//! SkillOpt-Sleep — the Onyx night cycle: transcripts -> digests -> tasks ->
//! per-skill consolidation reports.
//!
//! Deterministic by default. Passing a `ChatBackend` upgrades aggregation and
//! edit ranking from the fallback heuristics to model-driven merging; without
//! one, every stage still completes, which is what makes this usable on a box
//! with no reachable inference gateway.

use crate::skillopt::model::ChatBackend;
use crate::skillopt::optimizer::scheduler::{LRScheduler, SchedulerMode};
use crate::skillopt_sleep::onyx_consolidate::consolidate_once;
use crate::skillopt_sleep::onyx_harvest::harvest_onyx_sessions_dir;
use crate::skillopt_sleep::types::{SessionDigest, SkillGroupReport, TaskRecord};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct SleepCycleInput {
    /// Directory of Onyx transcripts, normally `$ONYX_HOME/sessions`.
    pub sessions_dir: std::path::PathBuf,
    /// Skills to optimize: name -> current instruction body.
    pub skills: HashMap<String, String>,
    pub max_sessions: usize,
    pub max_tasks: usize,
    pub merge_batch_size: usize,
    pub scheduler_mode: SchedulerMode,
    pub max_edits: usize,
    pub min_edits: usize,
    pub steps: usize,
    /// Optional workspace root to ingest Project Intelligence (WorkMemory & CoverageTracker).
    pub workspace_root: Option<std::path::PathBuf>,
}

impl Default for SleepCycleInput {
    fn default() -> Self {
        Self {
            sessions_dir: Path::new(".onyx").join("sessions"),
            skills: HashMap::new(),
            max_sessions: 200,
            max_tasks: 60,
            merge_batch_size: 4,
            scheduler_mode: SchedulerMode::Linear,
            max_edits: 6,
            min_edits: 1,
            steps: 3,
            workspace_root: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SleepCycleOutcome {
    pub digests: Vec<SessionDigest>,
    pub tasks: Vec<TaskRecord>,
    pub reports: Vec<SkillGroupReport>,
    /// New instruction body per skill that the gate accepted.
    pub updated_skills: HashMap<String, String>,
    pub notes: Vec<String>,
}

/// Assign mined tasks to skills. A task joins a skill when it came from a
/// session whose recorded title hints at that skill, or when only one skill is
/// in scope (the common single-skill optimization).
fn tasks_for_skill<'a>(tasks: &'a [TaskRecord], skill: &str, solo: bool) -> Vec<&'a TaskRecord> {
    let needle = skill.to_lowercase();
    tasks
        .iter()
        .filter(|t| solo || t.skill_hint.to_lowercase().contains(&needle) || !t.skill_hint.is_empty())
        .collect()
}

/// Run one full offline consolidation over every supplied skill.
pub async fn run_sleep_cycle(
    input: SleepCycleInput,
    backend: Option<&(dyn ChatBackend + Send + Sync)>,
) -> anyhow::Result<SleepCycleOutcome> {
    let mut out = SleepCycleOutcome::default();

    let digests = harvest_onyx_sessions_dir(&input.sessions_dir, input.max_sessions);
    if digests.is_empty() {
        out.notes.push(format!(
            "no transcripts under {}",
            input.sessions_dir.display()
        ));
        return Ok(out);
    }
    let n = digests.len();
    out.notes.push(format!("harvested {n} session transcript(s)"));

    let (work_mem, cov_tracker) = if let Some(ws) = &input.workspace_root {
        (
            Some(onyx_project::WorkMemory::load_or_init(ws)),
            Some(onyx_project::CoverageTracker::load_or_init(ws)),
        )
    } else {
        (None, None)
    };

    let tasks = crate::skillopt_sleep::mine::mine_with_project_intelligence(
        &digests,
        work_mem.as_ref(),
        cov_tracker.as_ref(),
        input.max_tasks,
    );
    if tasks.is_empty() {
        out.notes.push("miner found no tasks with a substantive first prompt".to_string());
    } else {
        out.notes.push(format!("mined {} recurring task record(s)", tasks.len()));
    }
    out.digests = digests;
    out.tasks = tasks.clone();

    let solo = input.skills.len() == 1;
    for (name, body) in &input.skills {
        let selected: Vec<TaskRecord> = tasks_for_skill(&out.tasks, name, solo)
            .into_iter()
            .cloned()
            .collect();
        if selected.is_empty() {
            out.reports.push(SkillGroupReport {
                skill_name: name.clone(),
                status: "no_tasks".to_string(),
                reason: "no mined tasks matched this skill".to_string(),
                ..Default::default()
            });
            continue;
        }

        // Multi-step budget decay: each step sees the skill as improved by the
        // previous accepted step, mirroring the trainer's rolling current/best.
        let mut current = body.clone();
        let mut baseline = {
            let results: Vec<_> = selected
                .iter()
                .map(|t| crate::skillopt_sleep::onyx_replay::replay_task_deterministic(t, &current))
                .collect();
            crate::skillopt_sleep::onyx_replay::aggregate(&results).0
        };
        let mut sched = LRScheduler::new(input.scheduler_mode, input.max_edits, input.min_edits, input.steps);
        let mut applied = 0usize;
        // The last step that the gate accepted. Reporting this (rather than a later
        // rejection) keeps the report consistent with `updated_skills`: if any step
        // improved the skill, the surfaced decision is that improvement.
        let mut last_accepted: Option<SkillGroupReport> = None;

        for step in 0..input.steps {
            let (mut report, candidate) =
                consolidate_once(name, &current, &selected, baseline, input.merge_batch_size, &mut sched, backend)
                    .await?;
            if report.accepted {
                current = candidate;
                baseline = report.candidate_score;
                applied += report.n_applied_edits;
                report.reason = format!("step {}: {}", step + 1, report.reason);
                last_accepted = Some(report);
            } else {
                // A rejected step leaves nothing further to try: stop, but surface
                // the best accepted step if there was one.
                report.reason = format!("step {}: {}", step + 1, report.reason);
                break;
            }
        }

        out.reports.push(match last_accepted {
            Some(r) => r,
            None => {
                // First step rejected outright: report that rejection verbatim.
                let results: Vec<_> = selected
                    .iter()
                    .map(|t| crate::skillopt_sleep::onyx_replay::replay_task_deterministic(t, &current))
                    .collect();
                let score = crate::skillopt_sleep::onyx_replay::aggregate(&results).0;
                SkillGroupReport {
                    skill_name: name.to_string(),
                    status: "rejected".to_string(),
                    accepted: false,
                    gate_action: "reject".to_string(),
                    baseline_score: score,
                    candidate_score: score,
                    n_tasks: selected.len(),
                    reason: "gate rejected all candidates, skill unchanged".to_string(),
                    ..Default::default()
                }
            }
        });

        if applied > 0 {
            out.updated_skills.insert(name.clone(), current);
            out.notes.push(format!("{name}: {applied} edit(s) accepted by the gate"));
        } else {
            out.notes.push(format!("{name}: gate rejected all candidates, skill unchanged"));
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    fn write_transcript(dir: &Path, id: &str, prompts: &[(&str, &str)]) {
        let path = dir.join(format!("{id}.jsonl"));
        let mut f = File::create(&path).unwrap();
        writeln!(f, r#"{{"kind":"meta","model":"m","ts":1700000000,"cwd":"/repo/x","branch":"main"}}"#).unwrap();
        for (user, assistant) in prompts {
            writeln!(f, r#"{{"kind":"message","role":"user","content":"{user}","ts":1700000001}}"#).unwrap();
            writeln!(f, r#"{{"kind":"message","role":"assistant","content":"{assistant}","ts":1700000002}}"#).unwrap();
        }
    }

    #[tokio::test]
    async fn cycle_runs_end_to_end_without_a_backend() {
        let dir = std::env::temp_dir().join(format!("onyx-cycle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_transcript(
            &dir,
            "1-a",
            &[
                ("Deploy the api service and check health", "still broken, deploy failed again"),
                ("Fix the flaky migration timeout", "wrong, revert it"),
            ],
        );

        let mut skills = HashMap::new();
        skills.insert("deploy".to_string(), "Ship the build.".to_string());

        let outcome = run_sleep_cycle(
            SleepCycleInput { sessions_dir: dir.clone(), skills, ..Default::default() },
            None,
        )
        .await
        .unwrap();

        assert_eq!(outcome.digests.len(), 1);
        assert!(!outcome.tasks.is_empty());
        assert!(!outcome.reports.is_empty());
        assert!(!outcome.notes.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn empty_store_reports_no_transcripts() {
        let dir = std::env::temp_dir().join(format!("onyx-cycle-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let outcome = run_sleep_cycle(
            SleepCycleInput { sessions_dir: dir.clone(), ..Default::default() },
            None,
        )
        .await
        .unwrap();
        assert!(outcome.digests.is_empty());
        assert!(outcome.notes[0].contains("no transcripts"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
