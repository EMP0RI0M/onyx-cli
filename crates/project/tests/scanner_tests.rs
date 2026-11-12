//! Unit and integration tests for onyx-project scanner.

use onyx_project::filesystem::FileFilter;
use onyx_project::jsts::JsTsExtractor;
use onyx_project::models::SymbolKind;
use onyx_project::rust_ast::RustAstExtractor;
use onyx_project::scanner::ProjectScanner;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_rust_ast_nested_modules_and_visibility() {
    let code = r#"
        pub mod outer {
            pub struct OuterStruct;
            fn private_fn() {}

            pub mod inner {
                pub fn inner_fn() -> i32 { 42 }
            }
        }

        pub fn root_fn() {}
        struct RootPrivate;
    "#;

    let (symbols, edges) = RustAstExtractor::extract("src/lib.rs", code).unwrap();

    // Check symbols
    assert!(symbols.iter().any(|s| s.name == "outer" && s.is_public && s.kind == SymbolKind::Module));
    assert!(symbols.iter().any(|s| s.name == "OuterStruct" && s.is_public && s.kind == SymbolKind::Struct));
    assert!(symbols.iter().any(|s| s.name == "private_fn" && !s.is_public && s.kind == SymbolKind::Function));
    assert!(symbols.iter().any(|s| s.name == "inner" && s.is_public && s.module_path.as_deref() == Some("outer")));
    assert!(symbols.iter().any(|s| s.name == "inner_fn" && s.is_public && s.module_path.as_deref() == Some("outer::inner")));
    assert!(symbols.iter().any(|s| s.name == "root_fn" && s.is_public));
    assert!(symbols.iter().any(|s| s.name == "RootPrivate" && !s.is_public));

    // Check module edges
    assert!(edges.iter().any(|e| e.to_target == "outer"));
}

#[test]
fn test_jsts_imports_and_exports() {
    let code = r#"
        import React, { useState } from 'react';
        import './styles.css';

        export function MyComponent() {
            return null;
        }

        export class ServiceClass {}
        export interface Props { id: string; }
        type ID = number;

        async function loadData() {
            const mod = await import('./dynamic-module');
        }
    "#;

    let (symbols, edges) = JsTsExtractor::extract("src/App.tsx", code);

    // Imports
    assert!(edges.iter().any(|e| e.to_target == "react" && !e.is_dynamic));
    assert!(edges.iter().any(|e| e.to_target == "./styles.css" && !e.is_dynamic));
    assert!(edges.iter().any(|e| e.to_target == "./dynamic-module" && e.is_dynamic));

    // Symbols
    assert!(symbols.iter().any(|s| s.name == "MyComponent" && s.is_public && s.kind == SymbolKind::Function));
    assert!(symbols.iter().any(|s| s.name == "ServiceClass" && s.is_public && s.kind == SymbolKind::Class));
    assert!(symbols.iter().any(|s| s.name == "Props" && s.is_public && s.kind == SymbolKind::Interface));
    assert!(symbols.iter().any(|s| s.name == "ID" && !s.is_public && s.kind == SymbolKind::TypeAlias));
}

#[test]
fn test_file_filtering_and_secret_exclusion() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    fs::write(root.join(".gitignore"), "ignored_dir/\n*.log\n").unwrap();

    let filter = FileFilter::new(root);

    // Directory exclusions
    assert!(filter.is_ignored("target/debug/build.rs", false));
    assert!(filter.is_ignored("node_modules/pkg/index.js", false));
    assert!(filter.is_ignored(".git/config", false));
    assert!(filter.is_ignored("ignored_dir/foo.rs", false));

    // Secret exclusions
    assert!(filter.is_ignored(".env", false));
    assert!(filter.is_ignored(".env.local", false));
    assert!(filter.is_ignored("secrets/id_rsa", false));
    assert!(filter.is_ignored("creds/credentials.json", false));

    // Allowed source files
    assert!(!filter.is_ignored("src/lib.rs", false));
    assert!(!filter.is_ignored("components/Button.tsx", false));
}

#[test]
fn test_incremental_scanning_and_atomic_save() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // 1. Setup sample Cargo workspace
    fs::write(root.join("Cargo.toml"), r#"
        [workspace]
        members = ["core"]
    "#).unwrap();

    fs::create_dir_all(root.join("core/src")).unwrap();
    fs::write(root.join("core/Cargo.toml"), r#"
        [package]
        name = "core"
        version = "0.1.0"
        [dependencies]
        serde = "1.0"
    "#).unwrap();

    let main_rs = root.join("core/src/lib.rs");
    fs::write(&main_rs, "pub fn original_fn() {}").unwrap();

    let scanner = ProjectScanner::new(root);

    // First scan: full
    let map1 = scanner.scan_full().unwrap();
    assert_eq!(map1.workspace.unwrap().members, vec!["core"]);
    assert!(map1.crates.contains_key("core"));
    assert!(map1.symbols.iter().any(|s| s.name == "original_fn"));

    // Verify architecture.json was created atomically
    assert!(scanner.output_path().exists());

    // 2. Modify one file and rescan incrementally
    fs::write(&main_rs, "pub fn updated_fn() {}\npub fn second_fn() {}").unwrap();
    let map2 = scanner.scan_incremental().unwrap();

    assert!(!map2.symbols.iter().any(|s| s.name == "original_fn"));
    assert!(map2.symbols.iter().any(|s| s.name == "updated_fn"));
    assert!(map2.symbols.iter().any(|s| s.name == "second_fn"));

    // 3. Delete file and rescan incrementally
    fs::remove_file(&main_rs).unwrap();
    let map3 = scanner.scan_incremental().unwrap();
    assert!(!map3.files.contains_key("core/src/lib.rs"));
    assert!(!map3.symbols.iter().any(|s| s.file == "core/src/lib.rs"));
}
