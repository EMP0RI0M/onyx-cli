//! Static configuration task extractors for monorepos and task runners.
//!
//! Parses package.json, nx.json/project.json, turbo.json, Makefile, and Justfile
//! to extract verifiable developer tasks without executing any external processes.

use std::collections::BTreeMap;
use std::path::Path;
use serde_json::Value;

use crate::models::{ProjectTaskProfile, TaskGoal};

/// Extract task profiles from all recognized manifests in a repository root.
pub fn extract_all_tasks<P: AsRef<Path>>(root: P) -> BTreeMap<String, ProjectTaskProfile> {
    let mut profiles = BTreeMap::new();
    let root = root.as_ref();

    // 1. package.json scripts
    let pkg_path = root.join("package.json");
    if pkg_path.is_file() {
        if let Ok(content) = std::fs::read_to_string(&pkg_path) {
            extract_package_json_scripts(&content, "package.json", &mut profiles);
        }
    }

    // 2. nx.json and project.json files
    let nx_path = root.join("nx.json");
    if nx_path.is_file() {
        if let Ok(content) = std::fs::read_to_string(&nx_path) {
            extract_nx_targets(&content, "nx.json", &mut profiles);
        }
    }

    // 3. turbo.json
    let turbo_path = root.join("turbo.json");
    if turbo_path.is_file() {
        if let Ok(content) = std::fs::read_to_string(&turbo_path) {
            extract_turbo_tasks(&content, "turbo.json", &mut profiles);
        }
    }

    // 4. Makefile
    let make_path = root.join("Makefile");
    if make_path.is_file() {
        if let Ok(content) = std::fs::read_to_string(&make_path) {
            extract_makefile_targets(&content, "Makefile", &mut profiles);
        }
    }

    // 5. Justfile
    let just_path = root.join("Justfile");
    let just_lower_path = root.join("justfile");
    let actual_just = if just_path.is_file() {
        Some(just_path)
    } else if just_lower_path.is_file() {
        Some(just_lower_path)
    } else {
        None
    };

    if let Some(p) = actual_just {
        if let Ok(content) = std::fs::read_to_string(&p) {
            extract_justfile_recipes(&content, "Justfile", &mut profiles);
        }
    }

    profiles
}

/// Infer formatter kind and goal from a shell command string.
fn infer_formatter_and_goal(raw: &str) -> Option<(&'static str, TaskGoal)> {
    let s = raw.to_lowercase();
    if s.contains("vitest") || s.contains("jest") || s.contains("playwright") {
        Some(("jest_vitest", TaskGoal::Test))
    } else if s.contains("pytest") || s.contains("python -m unittest") {
        Some(("pytest", TaskGoal::Test))
    } else if s.contains("cargo test") {
        Some(("cargo_test", TaskGoal::Test))
    } else if s.contains("cargo clippy") {
        Some(("cargo_clippy", TaskGoal::Clippy))
    } else if s.contains("cargo check") {
        Some(("cargo_check", TaskGoal::Check))
    } else if s.contains("cargo build") {
        Some(("cargo_check", TaskGoal::Build))
    } else if s.contains("eslint") || s.contains("biome") || s.contains("ruff") || s.contains("flake8") || s.contains("next lint") {
        Some(("jest_vitest", TaskGoal::Lint))
    } else if s.contains("next build") || s.contains("vite build") || s.contains("tsc") {
        Some(("cargo_check", TaskGoal::Build))
    } else if s.contains("next dev") || s.contains("vite") || s.contains("webpack") || s.contains("start") || s.contains("node") {
        Some(("raw", TaskGoal::Custom))
    } else {
        Some(("raw", TaskGoal::Custom))
    }
}

/// Extract tasks from package.json scripts.
pub fn extract_package_json_scripts(
    content: &str,
    source: &str,
    out: &mut BTreeMap<String, ProjectTaskProfile>,
) {
    let Ok(val) = serde_json::from_str::<Value>(content) else {
        return;
    };
    let Some(scripts) = val.get("scripts").and_then(|v| v.as_object()) else {
        return;
    };

    for (name, script_val) in scripts {
        let Some(script_str) = script_val.as_str() else {
            continue;
        };
        if let Some((fmt, goal)) = infer_formatter_and_goal(script_str) {
            // Register npm run <name>, pnpm run <name>, yarn <name>, bun run <name>
            let key = format!("npm run {name}");
            out.insert(
                key,
                ProjectTaskProfile {
                    runner: "npm".into(),
                    target: name.clone(),
                    formatter: fmt.into(),
                    goal,
                    source_file: source.into(),
                    raw_definition: script_str.into(),
                },
            );
            out.insert(
                format!("pnpm run {name}"),
                ProjectTaskProfile {
                    runner: "pnpm".into(),
                    target: name.clone(),
                    formatter: fmt.into(),
                    goal,
                    source_file: source.into(),
                    raw_definition: script_str.into(),
                },
            );
            out.insert(
                format!("pnpm {name}"),
                ProjectTaskProfile {
                    runner: "pnpm".into(),
                    target: name.clone(),
                    formatter: fmt.into(),
                    goal,
                    source_file: source.into(),
                    raw_definition: script_str.into(),
                },
            );
            out.insert(
                format!("yarn {name}"),
                ProjectTaskProfile {
                    runner: "yarn".into(),
                    target: name.clone(),
                    formatter: fmt.into(),
                    goal,
                    source_file: source.into(),
                    raw_definition: script_str.into(),
                },
            );
            out.insert(
                format!("bun run {name}"),
                ProjectTaskProfile {
                    runner: "bun".into(),
                    target: name.clone(),
                    formatter: fmt.into(),
                    goal,
                    source_file: source.into(),
                    raw_definition: script_str.into(),
                },
            );
        }
    }
}

/// Extract tasks from nx.json targetDefaults or project.json targets.
pub fn extract_nx_targets(
    content: &str,
    source: &str,
    out: &mut BTreeMap<String, ProjectTaskProfile>,
) {
    let Ok(val) = serde_json::from_str::<Value>(content) else {
        return;
    };

    // Case 1: nx.json targetDefaults
    if let Some(target_defs) = val.get("targetDefaults").and_then(|v| v.as_object()) {
        for (target_name, target_obj) in target_defs {
            let executor = target_obj.get("executor").and_then(|v| v.as_str()).unwrap_or("");
            let command = target_obj.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let raw = format!("{executor} {command}").trim().to_string();

            if let Some((fmt, goal)) = infer_formatter_and_goal(&raw) {
                out.insert(
                    format!("nx run {target_name}"),
                    ProjectTaskProfile {
                        runner: "nx".into(),
                        target: target_name.clone(),
                        formatter: fmt.into(),
                        goal,
                        source_file: source.into(),
                        raw_definition: raw.clone(),
                    },
                );
                out.insert(
                    format!("nx {target_name}"),
                    ProjectTaskProfile {
                        runner: "nx".into(),
                        target: target_name.clone(),
                        formatter: fmt.into(),
                        goal,
                        source_file: source.into(),
                        raw_definition: raw,
                    },
                );
            } else if target_name == "test" {
                out.insert(
                    "nx test".into(),
                    ProjectTaskProfile {
                        runner: "nx".into(),
                        target: "test".into(),
                        formatter: "jest_vitest".into(),
                        goal: TaskGoal::Test,
                        source_file: source.into(),
                        raw_definition: raw,
                    },
                );
            }
        }
    }

    // Case 2: project.json targets
    if let Some(targets) = val.get("targets").and_then(|v| v.as_object()) {
        let project_name = val.get("name").and_then(|v| v.as_str()).unwrap_or("");
        for (target_name, target_obj) in targets {
            let executor = target_obj.get("executor").and_then(|v| v.as_str()).unwrap_or("");
            let raw = executor.to_string();
            let (fmt, goal) = if executor.contains("jest") || executor.contains("vite") {
                ("jest_vitest", TaskGoal::Test)
            } else if executor.contains("lint") || executor.contains("eslint") {
                ("jest_vitest", TaskGoal::Lint)
            } else if target_name == "test" {
                ("jest_vitest", TaskGoal::Test)
            } else {
                continue;
            };

            let invocations = if project_name.is_empty() {
                vec![format!("nx run {target_name}"), format!("nx {target_name}")]
            } else {
                vec![
                    format!("nx run {project_name}:{target_name}"),
                    format!("nx {target_name} {project_name}"),
                ]
            };

            for inv in invocations {
                out.insert(
                    inv,
                    ProjectTaskProfile {
                        runner: "nx".into(),
                        target: target_name.clone(),
                        formatter: fmt.into(),
                        goal,
                        source_file: source.into(),
                        raw_definition: raw.clone(),
                    },
                );
            }
        }
    }
}

/// Extract tasks from turbo.json tasks or pipeline.
pub fn extract_turbo_tasks(
    content: &str,
    source: &str,
    out: &mut BTreeMap<String, ProjectTaskProfile>,
) {
    let Ok(val) = serde_json::from_str::<Value>(content) else {
        return;
    };

    let tasks_map = val.get("tasks")
        .or_else(|| val.get("pipeline"))
        .and_then(|v| v.as_object());

    if let Some(tasks) = tasks_map {
        for (task_name, _) in tasks {
            let bare_name = task_name.split('#').last().unwrap_or(task_name);
            let (fmt, goal) = if bare_name.contains("test") {
                ("jest_vitest", TaskGoal::Test)
            } else if bare_name.contains("check") {
                ("cargo_check", TaskGoal::Check)
            } else if bare_name.contains("lint") {
                ("jest_vitest", TaskGoal::Lint)
            } else {
                continue;
            };

            out.insert(
                format!("turbo run {task_name}"),
                ProjectTaskProfile {
                    runner: "turbo".into(),
                    target: task_name.clone(),
                    formatter: fmt.into(),
                    goal,
                    source_file: source.into(),
                    raw_definition: task_name.clone(),
                },
            );
            out.insert(
                format!("turbo {task_name}"),
                ProjectTaskProfile {
                    runner: "turbo".into(),
                    target: task_name.clone(),
                    formatter: fmt.into(),
                    goal,
                    source_file: source.into(),
                    raw_definition: task_name.clone(),
                },
            );
        }
    }
}

/// Extract targets from Makefile recipes.
pub fn extract_makefile_targets(
    content: &str,
    source: &str,
    out: &mut BTreeMap<String, ProjectTaskProfile>,
) {
    let mut current_target: Option<String> = None;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }

        if !line.starts_with('\t') && !line.starts_with("    ") && line.contains(':') && !line.contains(":=") {
            if let Some((target, _)) = line.split_once(':') {
                let target = target.trim();
                if !target.starts_with('.') && !target.contains('$') {
                    current_target = Some(target.to_string());
                }
            }
        } else if let Some(target) = &current_target {
            // First recipe line of this target
            if let Some((fmt, goal)) = infer_formatter_and_goal(trimmed) {
                out.insert(
                    format!("make {target}"),
                    ProjectTaskProfile {
                        runner: "make".into(),
                        target: target.clone(),
                        formatter: fmt.into(),
                        goal,
                        source_file: source.into(),
                        raw_definition: trimmed.into(),
                    },
                );
            }
            current_target = None;
        }
    }
}

/// Extract recipes from Justfile.
pub fn extract_justfile_recipes(
    content: &str,
    source: &str,
    out: &mut BTreeMap<String, ProjectTaskProfile>,
) {
    let mut current_target: Option<String> = None;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }

        if !line.starts_with('\t') && !line.starts_with("    ") && line.contains(':') && !line.contains(":=") {
            if let Some((recipe, _)) = line.split_once(':') {
                let recipe = recipe.trim();
                if !recipe.starts_with('@') && !recipe.contains('$') {
                    current_target = Some(recipe.to_string());
                }
            }
        } else if let Some(recipe) = &current_target {
            if let Some((fmt, goal)) = infer_formatter_and_goal(trimmed) {
                out.insert(
                    format!("just {recipe}"),
                    ProjectTaskProfile {
                        runner: "just".into(),
                        target: recipe.clone(),
                        formatter: fmt.into(),
                        goal,
                        source_file: source.into(),
                        raw_definition: trimmed.into(),
                    },
                );
            }
            current_target = None;
        }
    }
}
