use onyx_project::scanner::ProjectScanner;
use std::time::Instant;

#[test]
fn test_scan_actual_tektile_core() {
    let root = std::path::Path::new("/root/tektile-core");
    let scanner = ProjectScanner::new(root);
    let start = Instant::now();
    let map = scanner.scan_full().unwrap();
    let elapsed = start.elapsed();

    println!("Scanned tektile-core in {:?}", elapsed);
    println!("Total files cataloged: {}", map.files.len());
    println!("Total symbols extracted: {}", map.symbols.len());
    println!("Total dependency edges: {}", map.dependencies.len());

    assert!(map.files.len() > 50);
    assert!(map.symbols.len() > 500);
}
