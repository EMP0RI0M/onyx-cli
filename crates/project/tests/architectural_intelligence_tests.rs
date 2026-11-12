use onyx_project::{
    ArchitectureMap, CoverageTracker, DependencyEdge, EdgeKind, FileMeta, GapConfidence,
    GapKind, ImpactAnalyzer, IntegrationGapDetector, SymbolKind, SymbolMeta, SymbolRetriever,
    WorkMemory,
};

fn create_sample_architecture() -> ArchitectureMap {
    let mut arch = ArchitectureMap::new("/repo");

    // Files
    arch.files.insert(
        "src/lib.rs".to_string(),
        FileMeta {
            path: "src/lib.rs".to_string(),
            language: "rust".to_string(),
            sha256: "hash_lib".to_string(),
            size_bytes: 500,
            last_scanned_at: 100,
        },
    );
    arch.files.insert(
        "src/core.rs".to_string(),
        FileMeta {
            path: "src/core.rs".to_string(),
            language: "rust".to_string(),
            sha256: "hash_core".to_string(),
            size_bytes: 1200,
            last_scanned_at: 100,
        },
    );
    arch.files.insert(
        "src/util.rs".to_string(),
        FileMeta {
            path: "src/util.rs".to_string(),
            language: "rust".to_string(),
            sha256: "hash_util".to_string(),
            size_bytes: 300,
            last_scanned_at: 100,
        },
    );
    arch.files.insert(
        "src/orphan.rs".to_string(),
        FileMeta {
            path: "src/orphan.rs".to_string(),
            language: "rust".to_string(),
            sha256: "hash_orphan".to_string(),
            size_bytes: 400,
            last_scanned_at: 100,
        },
    );
    arch.files.insert(
        "tests/core_test.rs".to_string(),
        FileMeta {
            path: "tests/core_test.rs".to_string(),
            language: "rust".to_string(),
            sha256: "hash_test".to_string(),
            size_bytes: 800,
            last_scanned_at: 100,
        },
    );

    // Dependencies:
    // lib -> core
    arch.dependencies.push(DependencyEdge {
        from_file: "src/lib.rs".to_string(),
        to_target: "src/core.rs".to_string(),
        kind: EdgeKind::Import,
        is_dynamic: false,
        confidence: 1.0,
    });
    // core -> util
    arch.dependencies.push(DependencyEdge {
        from_file: "src/core.rs".to_string(),
        to_target: "src/util.rs".to_string(),
        kind: EdgeKind::Import,
        is_dynamic: false,
        confidence: 1.0,
    });
    // test -> core
    arch.dependencies.push(DependencyEdge {
        from_file: "tests/core_test.rs".to_string(),
        to_target: "src/core.rs".to_string(),
        kind: EdgeKind::Import,
        is_dynamic: false,
        confidence: 1.0,
    });

    // Symbols
    arch.symbols.push(SymbolMeta {
        file: "src/core.rs".to_string(),
        name: "EngineCore".to_string(),
        kind: SymbolKind::Struct,
        is_public: true,
        line: 10,
        module_path: Some("core".to_string()),
        signature: Some("pub struct EngineCore".to_string()),
        conditional: None,
    });
    arch.symbols.push(SymbolMeta {
        file: "src/core.rs".to_string(),
        name: "compute_flow".to_string(),
        kind: SymbolKind::Function,
        is_public: true,
        line: 25,
        module_path: Some("core".to_string()),
        signature: Some("pub fn compute_flow()".to_string()),
        conditional: None,
    });
    arch.symbols.push(SymbolMeta {
        file: "tests/core_test.rs".to_string(),
        name: "test_compute_flow".to_string(),
        kind: SymbolKind::Function,
        is_public: false,
        line: 5,
        module_path: None,
        signature: Some("fn test_compute_flow()".to_string()),
        conditional: None,
    });

    arch
}

#[test]
fn test_integration_gap_detection_orphans() {
    let arch = create_sample_architecture();
    let detector = IntegrationGapDetector::new(&arch);

    let gaps = detector.detect_orphaned_modules();
    assert!(!gaps.is_empty());

    let orphan = gaps.iter().find(|g| g.affected_file == "src/orphan.rs");
    assert!(orphan.is_some(), "src/orphan.rs has 0 inbound edges and must be flagged as orphan");
    assert_eq!(orphan.unwrap().kind, GapKind::OrphanedModule);
    assert_eq!(orphan.unwrap().confidence, GapConfidence::StrongCandidate);

    // Verify lib.rs and tests/core_test.rs are recognized as entrypoints and not false-positive orphans
    assert!(!gaps.iter().any(|g| g.affected_file == "src/lib.rs"));
    assert!(!gaps.iter().any(|g| g.affected_file == "tests/core_test.rs"));
}

#[test]
fn test_circular_dependency_detection() {
    let mut arch = create_sample_architecture();
    // Add cycle: util -> lib
    arch.dependencies.push(DependencyEdge {
        from_file: "src/util.rs".to_string(),
        to_target: "src/lib.rs".to_string(),
        kind: EdgeKind::Import,
        is_dynamic: false,
        confidence: 1.0,
    });

    let detector = IntegrationGapDetector::new(&arch);
    let cycles = detector.detect_circular_dependencies();

    assert!(!cycles.is_empty(), "Cycle lib -> core -> util -> lib must be detected");
    assert_eq!(cycles[0].kind, GapKind::CircularDependency);
    assert_eq!(cycles[0].confidence, GapConfidence::Confirmed);
}

#[test]
fn test_impact_analysis_propagation() {
    let arch = create_sample_architecture();
    let analyzer = ImpactAnalyzer::new(&arch);

    // Editing util.rs:
    // Direct dependent: core.rs
    // Transitive (indirect) dependent: lib.rs, core_test.rs
    let report = analyzer.analyze_impact("src/util.rs", None);

    assert_eq!(report.target_file, "src/util.rs");
    assert!(report.direct_dependents.contains(&"src/core.rs".to_string()));
    assert!(report.indirect_dependents.contains(&"src/lib.rs".to_string()));
    assert!(report.related_tests.contains(&"tests/core_test.rs".to_string()));
    assert!(report.risk_score > 0.1, "Risk score must reflect non-leaf blast radius");
}

#[test]
fn test_ast_aware_symbol_retrieval_and_tests() {
    let arch = create_sample_architecture();
    let retriever = SymbolRetriever::new(&arch);

    // 1. Retrieve symbols
    let results = retriever.retrieve_symbols("EngineCore compute flow", 5);
    assert!(!results.is_empty());
    assert_eq!(results[0].name, "EngineCore");
    assert_eq!(results[0].line, 10);
    assert_eq!(results[0].file, "src/core.rs");

    // 2. Associated tests
    let tests = retriever.find_associated_tests("compute_flow");
    assert_eq!(tests.len(), 1);
    assert_eq!(tests[0].name, "test_compute_flow");
    assert_eq!(tests[0].file, "tests/core_test.rs");

    // 3. Find callers
    let callers = retriever.find_callers_of_symbol("src/core.rs");
    assert!(callers.contains(&"src/lib.rs".to_string()));
}
