use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::models::{ArchitectureMap, SymbolKind};

/// Contextually retrieved code symbol with exact source location.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievedSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub file: String,
    pub line: usize,
    pub signature: Option<String>,
    pub score: f32,
    pub is_test: bool,
}

/// In-memory lexical code index for AST symbols and structural elements.
pub struct SymbolRetriever<'a> {
    pub arch: &'a ArchitectureMap,
}

impl<'a> SymbolRetriever<'a> {
    pub fn new(arch: &'a ArchitectureMap) -> Self {
        Self { arch }
    }

    /// Retrieve top matching symbols for a natural language or identifier query.
    pub fn retrieve_symbols(&self, query: &str, limit: usize) -> Vec<RetrievedSymbol> {
        let terms: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|w| w.len() >= 3)
            .map(|w| w.to_lowercase())
            .collect();

        if terms.is_empty() {
            return Vec::new();
        }

        let mut scored: Vec<RetrievedSymbol> = Vec::new();

        for sym in &self.arch.symbols {
            let mut score = 0.0f32;
            let name_lower = sym.name.to_lowercase();
            let file_lower = sym.file.to_lowercase();
            let sig_lower = sym.signature.as_ref().map(|s| s.to_lowercase()).unwrap_or_default();

            for term in &terms {
                // Exact symbol name match
                if name_lower == *term {
                    score += 15.0;
                } else if name_lower.contains(term) {
                    score += 6.0;
                }

                // File path match
                if file_lower.contains(term) {
                    score += 3.0;
                }

                // Signature match
                if sig_lower.contains(term) {
                    score += 2.0;
                }
            }

            // Boost public symbols
            if sym.is_public {
                score *= 1.2;
            }

            if score > 0.0 {
                let is_test = sym.file.contains("test") || sym.name.starts_with("test_");
                scored.push(RetrievedSymbol {
                    name: sym.name.clone(),
                    kind: sym.kind,
                    file: sym.file.clone(),
                    line: sym.line,
                    signature: sym.signature.clone(),
                    score,
                    is_test,
                });
            }
        }

        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        scored
    }

    /// Find callers or consumers of a specific symbol across the repository.
    pub fn find_callers_of_symbol(&self, symbol_name: &str) -> Vec<String> {
        let mut callers = HashSet::new();
        for edge in &self.arch.dependencies {
            if edge.to_target.contains(symbol_name) {
                callers.insert(edge.from_file.clone());
            }
        }
        for sym in &self.arch.symbols {
            if let Some(sig) = &sym.signature {
                if sig.contains(symbol_name) && sym.name != symbol_name {
                    callers.insert(sym.file.clone());
                }
            }
        }
        let mut list: Vec<_> = callers.into_iter().collect();
        list.sort();
        list
    }

    /// Retrieve associated unit tests for a symbol or file.
    pub fn find_associated_tests(&self, symbol_or_file: &str) -> Vec<RetrievedSymbol> {
        let mut tests = Vec::new();
        let target_lower = symbol_or_file.to_lowercase();

        for sym in &self.arch.symbols {
            let is_test = sym.file.contains("test") || sym.name.starts_with("test_");
            if is_test {
                if sym.name.to_lowercase().contains(&target_lower)
                    || sym.file.to_lowercase().contains(&target_lower)
                {
                    tests.push(RetrievedSymbol {
                        name: sym.name.clone(),
                        kind: sym.kind,
                        file: sym.file.clone(),
                        line: sym.line,
                        signature: sym.signature.clone(),
                        score: 1.0,
                        is_test: true,
                    });
                }
            }
        }
        tests
    }
}
