//! Cargo metadata and workspace structure discovery.

use crate::models::{CrateMeta, DependencyEdge, EdgeKind, WorkspaceMeta};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

pub struct CargoScanner;

impl CargoScanner {
    pub fn scan_workspace(root: &Path) -> (Option<WorkspaceMeta>, BTreeMap<String, CrateMeta>, Vec<DependencyEdge>) {
        let cargo_toml = root.join("Cargo.toml");
        if !cargo_toml.exists() {
            return (None, BTreeMap::new(), Vec::new());
        }

        let Ok(content) = fs::read_to_string(&cargo_toml) else {
            return (None, BTreeMap::new(), Vec::new());
        };

        let mut members = Vec::new();
        let mut crates = BTreeMap::new();
        let mut edges = Vec::new();

        // Simple TOML line parser to avoid heavy toml crate if not needed
        let mut in_workspace_members = false;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("members") && trimmed.contains('[') {
                in_workspace_members = true;
                // Parse members array
                if let Some(start) = trimmed.find('[') {
                    if let Some(end) = trimmed.rfind(']') {
                        let inner = &trimmed[start + 1..end];
                        for m in inner.split(',') {
                            let clean = m.trim().trim_matches('"').trim_matches('\'');
                            if !clean.is_empty() && !clean.starts_with('[') {
                                members.push(clean.to_string());
                            }
                        }
                        in_workspace_members = false;
                    }
                }
            } else if in_workspace_members {
                if trimmed.contains(']') {
                    in_workspace_members = false;
                }
                for m in trimmed.split(',') {
                    let clean = m.trim().trim_matches('"').trim_matches('\'').trim_matches(']').trim_matches('[');
                    if !clean.is_empty() && !clean.starts_with('[') {
                        members.push(clean.to_string());
                    }
                }
            }
        }

        // If no explicit workspace members found, treat root as single crate
        if members.is_empty() {
            if let Some(krate) = Self::scan_single_crate(&cargo_toml, "") {
                for dep in &krate.dependencies {
                    edges.push(DependencyEdge {
                        from_file: "Cargo.toml".to_string(),
                        to_target: dep.clone(),
                        kind: EdgeKind::CrateDep,
                        is_dynamic: false,
                        confidence: 1.0,
                    });
                }
                crates.insert(krate.name.clone(), krate);
            }
        } else {
            for m in &members {
                let member_cargo = root.join(m).join("Cargo.toml");
                if let Some(krate) = Self::scan_single_crate(&member_cargo, m) {
                    for dep in &krate.dependencies {
                        edges.push(DependencyEdge {
                            from_file: format!("{m}/Cargo.toml"),
                            to_target: dep.clone(),
                            kind: EdgeKind::CrateDep,
                            is_dynamic: false,
                            confidence: 1.0,
                        });
                    }
                    crates.insert(krate.name.clone(), krate);
                }
            }
        }

        let ws = Some(WorkspaceMeta {
            kind: "cargo".to_string(),
            members,
        });

        (ws, crates, edges)
    }

    fn scan_single_crate(manifest_path: &Path, rel_path: &str) -> Option<CrateMeta> {
        let content = fs::read_to_string(manifest_path).ok()?;
        let mut name = None;
        let mut version = None;
        let mut dependencies = Vec::new();

        let mut in_package = false;
        let mut in_deps = false;

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed == "[package]" {
                in_package = true;
                in_deps = false;
                continue;
            } else if trimmed.starts_with('[') {
                in_package = false;
                in_deps = trimmed == "[dependencies]" || trimmed == "[workspace.dependencies]";
                continue;
            }

            if in_package {
                if trimmed.starts_with("name") {
                    if let Some(val) = trimmed.split('=').nth(1) {
                        name = Some(val.trim().trim_matches('"').trim_matches('\'').to_string());
                    }
                } else if trimmed.starts_with("version") {
                    if let Some(val) = trimmed.split('=').nth(1) {
                        version = Some(val.trim().trim_matches('"').trim_matches('\'').to_string());
                    }
                }
            } else if in_deps {
                if let Some(key) = trimmed.split('=').next() {
                    let dep_name = key.trim();
                    if !dep_name.is_empty() && !dep_name.starts_with('#') {
                        dependencies.push(dep_name.to_string());
                    }
                }
            }
        }

        name.map(|n| CrateMeta {
            name: n,
            version,
            path: rel_path.to_string(),
            dependencies,
        })
    }
}
