//! JavaScript and TypeScript structural scanner for imports, exports, functions, classes, and types.

use crate::models::{DependencyEdge, EdgeKind, SymbolKind, SymbolMeta};
use regex::Regex;

pub struct JsTsExtractor;

impl JsTsExtractor {
    pub fn extract(file_path: &str, content: &str) -> (Vec<SymbolMeta>, Vec<DependencyEdge>) {
        let mut symbols = Vec::new();
        let mut edges = Vec::new();

        // Regex patterns for JS/TS
        // 1. Static imports: import ... from '...'; or import '...';
        let import_re = Regex::new(r#"import\s+(?:(?:[\w*\s{},$]+)\s+from\s+)?['"]([^'"]+)['"]"#).unwrap();
        // 2. Dynamic imports: import('...')
        let dynamic_import_re = Regex::new(r#"import\s*\(\s*['"]([^'"]+)['"]\s*\)"#).unwrap();
        // 3. Export default / named
        let export_named_re = Regex::new(r#"export\s+(?:default\s+)?(?:async\s+)?(function|class|interface|type|const|let|var)\s+([a-zA-Z0-9_$]+)"#).unwrap();
        // 4. Non-exported top-level declarations
        let decl_re = Regex::new(r#"^(?:async\s+)?(function|class|interface|type)\s+([a-zA-Z0-9_$]+)"#).unwrap();

        for (idx, line) in content.lines().enumerate() {
            let line_no = idx + 1;
            let trimmed = line.trim();

            // Skip comments
            if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') {
                continue;
            }

            // Static imports
            if let Some(caps) = import_re.captures(trimmed) {
                if let Some(target) = caps.get(1) {
                    edges.push(DependencyEdge {
                        from_file: file_path.to_string(),
                        to_target: target.as_str().to_string(),
                        kind: EdgeKind::Import,
                        is_dynamic: false,
                        confidence: 1.0,
                    });
                }
            }

            // Dynamic imports
            if let Some(caps) = dynamic_import_re.captures(trimmed) {
                if let Some(target) = caps.get(1) {
                    edges.push(DependencyEdge {
                        from_file: file_path.to_string(),
                        to_target: target.as_str().to_string(),
                        kind: EdgeKind::Import,
                        is_dynamic: true,
                        confidence: 0.9,
                    });
                }
            }

            // Exported symbols
            if let Some(caps) = export_named_re.captures(trimmed) {
                let kind_str = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let name = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                let kind = match kind_str {
                    "function" => SymbolKind::Function,
                    "class" => SymbolKind::Class,
                    "interface" => SymbolKind::Interface,
                    "type" => SymbolKind::TypeAlias,
                    _ => SymbolKind::Export,
                };
                symbols.push(SymbolMeta {
                    file: file_path.to_string(),
                    name: name.to_string(),
                    kind,
                    is_public: true,
                    line: line_no,
                    module_path: None,
                    signature: Some(trimmed.to_string()),
                    conditional: None,
                });
            } else if let Some(caps) = decl_re.captures(trimmed) {
                // Non-exported declarations
                let kind_str = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let name = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                let kind = match kind_str {
                    "function" => SymbolKind::Function,
                    "class" => SymbolKind::Class,
                    "interface" => SymbolKind::Interface,
                    "type" => SymbolKind::TypeAlias,
                    _ => SymbolKind::Constant,
                };
                symbols.push(SymbolMeta {
                    file: file_path.to_string(),
                    name: name.to_string(),
                    kind,
                    is_public: false,
                    line: line_no,
                    module_path: None,
                    signature: Some(trimmed.to_string()),
                    conditional: None,
                });
            }
        }

        (symbols, edges)
    }
}
