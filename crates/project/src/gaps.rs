use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::models::{ArchitectureMap, EdgeKind};

/// Confidence in an architectural finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapConfidence {
    Confirmed,
    StrongCandidate,
    Possible,
    Unknown,
}

/// Category of detected architectural gap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapKind {
    /// File or module has no incoming references or dependencies from any entrypoint or other module.
    OrphanedModule,
    /// Trait or interface declared with no known implementations or usages.
    UnusedInterface,
    /// Public symbol declared with no references in any other file or module.
    UnreferencedPublicSymbol,
    /// Two or more functions/types share an identical signature or name across modules.
    DuplicateImplementation,
    /// Direct or indirect dependency cycle between modules.
    CircularDependency,
    /// File is cataloged in the repository but disconnected from all package manifests.
    DisconnectedFromRuntime,
}

/// Specific architectural gap detected in the codebase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationGap {
    pub kind: GapKind,
    pub title: String,
    pub description: String,
    pub affected_file: String,
    pub symbol_name: Option<String>,
    pub confidence: GapConfidence,
    pub mitigation_hint: String,
}

/// Analyzer for detecting integration gaps and anomalies in the architecture graph.
pub struct IntegrationGapDetector<'a> {
    pub arch: &'a ArchitectureMap,
}

impl<'a> IntegrationGapDetector<'a> {
    pub fn new(arch: &'a ArchitectureMap) -> Self {
        Self { arch }
    }

    /// Run full gap analysis across the indexed architecture map.
    pub fn detect_all_gaps(&self) -> Vec<IntegrationGap> {
        let mut gaps = Vec::new();
        gaps.extend(self.detect_orphaned_modules());
        gaps.extend(self.detect_circular_dependencies());
        gaps.extend(self.detect_unused_interfaces());
        gaps.extend(self.detect_unreferenced_public_symbols());
        gaps.extend(self.detect_duplicate_implementations());
        gaps
    }

    /// Detect files that have no inbound dependency edges and are not standard root entrypoints.
    pub fn detect_orphaned_modules(&self) -> Vec<IntegrationGap> {
        let mut inbound_count: HashMap<&str, usize> = HashMap::new();
        for file in self.arch.files.keys() {
            inbound_count.insert(file.as_str(), 0);
        }

        for edge in &self.arch.dependencies {
            // Count inbound references to target
            for file in self.arch.files.keys() {
                if edge.to_target.contains(file.as_str()) || file.ends_with(&edge.to_target) {
                    *inbound_count.entry(file.as_str()).or_insert(0) += 1;
                }
            }
        }

        let mut gaps = Vec::new();
        for (file, count) in inbound_count {
            if count == 0 {
                // Ignore standard workspace entrypoints
                if is_standard_entrypoint(file) {
                    continue;
                }

                // Confidence adjustment: dynamic imports or tests
                let confidence = if file.contains("test") || file.contains("example") || file.contains("bench") {
                    GapConfidence::Possible
                } else if file.ends_with(".rs") || file.ends_with(".ts") || file.ends_with(".js") {
                    GapConfidence::StrongCandidate
                } else {
                    GapConfidence::Possible
                };

                gaps.push(IntegrationGap {
                    kind: GapKind::OrphanedModule,
                    title: format!("Potentially orphaned file `{}`", file),
                    description: format!(
                        "File `{}` has 0 incoming dependency edges in the architecture map.",
                        file
                    ),
                    affected_file: file.to_string(),
                    symbol_name: None,
                    confidence,
                    mitigation_hint: "Verify if this module is registered in a parent `mod.rs`/`lib.rs`, imported dynamically, or dead code.".into(),
                });
            }
        }

        gaps
    }

    /// Detect circular dependencies (A -> B -> ... -> A) among files.
    pub fn detect_circular_dependencies(&self) -> Vec<IntegrationGap> {
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
        for edge in &self.arch.dependencies {
            if edge.kind == EdgeKind::Import || edge.kind == EdgeKind::ModuleChild {
                adj.entry(edge.from_file.as_str())
                    .or_default()
                    .push(edge.to_target.as_str());
            }
        }

        let mut gaps = Vec::new();
        let mut visited: HashSet<&str> = HashSet::new();
        let mut rec_stack: Vec<&str> = Vec::new();

        for node in adj.keys() {
            if !visited.contains(node) {
                let mut cycles: Vec<Vec<&str>> = Vec::new();
                dfs_cycle(*node, &adj, &mut visited, &mut rec_stack, &mut cycles);

                for cycle in cycles {
                    if cycle.len() >= 2 {
                        let cycle_str = cycle.join(" -> ");
                        gaps.push(IntegrationGap {
                            kind: GapKind::CircularDependency,
                            title: format!("Circular dependency detected: {}", cycle_str),
                            description: format!(
                                "Cycle detected between components: {}. This can cause compilation or initialization deadlocks.",
                                cycle_str
                            ),
                            affected_file: cycle[0].to_string(),
                            symbol_name: None,
                            confidence: GapConfidence::Confirmed,
                            mitigation_hint: "Break the cycle by extracting shared types into a common leaf module or using dependency inversion.".into(),
                        });
                    }
                }
            }
        }

        gaps
    }

    /// Detect interfaces or traits with no implementations.
    pub fn detect_unused_interfaces(&self) -> Vec<IntegrationGap> {
        let mut interfaces: HashMap<&str, &str> = HashMap::new(); // name -> file
        let mut impls: HashSet<&str> = HashSet::new();

        for sym in &self.arch.symbols {
            if sym.kind == crate::models::SymbolKind::Trait || sym.kind == crate::models::SymbolKind::Interface {
                interfaces.insert(sym.name.as_str(), sym.file.as_str());
            } else if sym.kind == crate::models::SymbolKind::Impl {
                impls.insert(sym.name.as_str());
            }
        }

        let mut gaps = Vec::new();
        for (iface, file) in interfaces {
            let has_impl = impls.iter().any(|i| i.contains(iface));
            if !has_impl {
                // Check if name is referenced anywhere in dependencies
                let has_ref = self.arch.dependencies.iter().any(|d| d.to_target.contains(iface));
                if !has_ref {
                    gaps.push(IntegrationGap {
                        kind: GapKind::UnusedInterface,
                        title: format!("Interface/Trait `{}` has no local implementations", iface),
                        description: format!(
                            "Trait/Interface `{}` declared in `{}` has no detected `impl` blocks in the codebase.",
                            iface, file
                        ),
                        affected_file: file.to_string(),
                        symbol_name: Some(iface.to_string()),
                        confidence: GapConfidence::StrongCandidate,
                        mitigation_hint: "Verify if this interface is implemented in an external crate or intended as an extensibility SPI.".into(),
                    });
                }
            }
        }

        gaps
    }

    /// Detect public functions or types never referenced outside their defining file.
    pub fn detect_unreferenced_public_symbols(&self) -> Vec<IntegrationGap> {
        let mut gaps = Vec::new();
        let pub_symbols: Vec<_> = self.arch.symbols.iter().filter(|s| s.is_public).collect();

        // Sample up to 50 public symbols for fast checking
        for sym in pub_symbols.iter().take(50) {
            if sym.name == "new" || sym.name == "default" || sym.name == "main" || sym.name.starts_with("test_") {
                continue;
            }

            // Check if any other file imports or references this symbol name
            let is_referenced = self.arch.symbols.iter().any(|other| {
                other.file != sym.file && other.signature.as_ref().map(|s| s.contains(&sym.name)).unwrap_or(false)
            }) || self.arch.dependencies.iter().any(|dep| {
                dep.from_file != sym.file && dep.to_target.contains(&sym.name)
            });

            if !is_referenced {
                gaps.push(IntegrationGap {
                    kind: GapKind::UnreferencedPublicSymbol,
                    title: format!("Public symbol `{}` appears unreferenced", sym.name),
                    description: format!(
                        "Public {:?} `{}` in `{}` has no direct consumers found in other repository files.",
                        sym.kind, sym.name, sym.file
                    ),
                    affected_file: sym.file.clone(),
                    symbol_name: Some(sym.name.clone()),
                    confidence: GapConfidence::Possible,
                    mitigation_hint: "Check whether this is part of a public crate library API or can be made private (`pub(crate)`).".into(),
                });
            }
        }

        gaps
    }

    /// Detect duplicate or overlapping symbol names across different files.
    pub fn detect_duplicate_implementations(&self) -> Vec<IntegrationGap> {
        let mut by_name: HashMap<&str, Vec<&str>> = HashMap::new(); // name -> Vec<file>

        for sym in &self.arch.symbols {
            if sym.name.len() >= 4 && sym.kind != crate::models::SymbolKind::Impl && sym.kind != crate::models::SymbolKind::Module {
                by_name.entry(sym.name.as_str()).or_default().push(sym.file.as_str());
            }
        }

        let mut gaps = Vec::new();
        for (name, files) in by_name {
            let unique_files: HashSet<_> = files.iter().copied().collect();
            if unique_files.len() > 1 && !is_common_trait_method(name) {
                let file_list = unique_files.into_iter().collect::<Vec<_>>().join(", ");
                gaps.push(IntegrationGap {
                    kind: GapKind::DuplicateImplementation,
                    title: format!("Duplicate symbol name `{}` in multiple files", name),
                    description: format!(
                        "Symbol `{}` is implemented across multiple separate files: [{}].",
                        name, file_list
                    ),
                    affected_file: files[0].to_string(),
                    symbol_name: Some(name.to_string()),
                    confidence: GapConfidence::Possible,
                    mitigation_hint: "Verify if these represent redundant implementations that can be consolidated into a shared utility.".into(),
                });
            }
        }

        gaps
    }
}

fn dfs_cycle<'a>(
    node: &'a str,
    adj: &HashMap<&'a str, Vec<&'a str>>,
    visited: &mut HashSet<&'a str>,
    stack: &mut Vec<&'a str>,
    cycles: &mut Vec<Vec<&'a str>>,
) {
    visited.insert(node);
    stack.push(node);

    if let Some(neighbors) = adj.get(node) {
        for &neighbor in neighbors {
            if let Some(idx) = stack.iter().position(|&s| s == neighbor) {
                // Cycle detected
                let mut cycle = stack[idx..].to_vec();
                cycle.push(neighbor);
                cycles.push(cycle);
            } else if !visited.contains(neighbor) {
                dfs_cycle(neighbor, adj, visited, stack, cycles);
            }
        }
    }

    stack.pop();
}

fn is_standard_entrypoint(path: &str) -> bool {
    path.ends_with("main.rs")
        || path.ends_with("lib.rs")
        || path.ends_with("index.ts")
        || path.ends_with("index.js")
        || path.ends_with("Cargo.toml")
        || path.ends_with("package.json")
        || path.contains("tests/")
        || path.contains("examples/")
        || path.contains("benches/")
}

fn is_common_trait_method(name: &str) -> bool {
    matches!(
        name,
        "new"
            | "default"
            | "clone"
            | "from"
            | "into"
            | "fmt"
            | "drop"
            | "run"
            | "execute"
            | "step"
            | "call"
            | "init"
            | "start"
            | "stop"
    )
}
