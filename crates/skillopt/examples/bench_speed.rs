use std::time::Instant;
use skillopt::skillopt::utils::scoring::skill_hash;
use skillopt::skillopt::evaluation::gate::{evaluate_gate, GateOptions, GateMetric};
use skillopt::skillopt::engine::ReflActPipeline;
use skillopt::skillopt::types::{Patch, Edit, EditOp};

fn main() {
    let doc = "# Test Skill\n".repeat(50);
    let patch = Patch {
        edits: vec![
            Edit { op: EditOp::Append, content: "Rule 1".into(), ..Default::default() },
            Edit { op: EditOp::Replace, target: "# Test Skill".into(), content: "# Verified Skill".into(), ..Default::default() },
        ],
        ..Default::default()
    };

    // 1. Skill Hash Bench
    let start = Instant::now();
    for _ in 0..50000 {
        let _ = skill_hash(&doc);
    }
    let d_hash = start.elapsed();
    println!("Rust Hashing (50,000 runs): {:.2?} ({:.0} ops/sec)", d_hash, 50000.0 / d_hash.as_secs_f64());

    // 2. Patch & AST Mutator Bench
    let start = Instant::now();
    for _ in 0..50000 {
        let _ = ReflActPipeline::apply_patch(&doc, &patch);
    }
    let d_patch = start.elapsed();
    println!("Rust Patch Application (50,000 runs): {:.2?} ({:.0} ops/sec)", d_patch, 50000.0 / d_patch.as_secs_f64());

    // 3. Gate Evaluation Bench
    let opts = GateOptions { metric: GateMetric::Mixed, mixed_weight: 0.5, ..Default::default() };
    let start = Instant::now();
    for i in 0..50000 {
        let _ = evaluate_gate("cand", 0.8, "curr", 0.6, "best", 0.7, 0, i, opts.clone());
    }
    let d_gate = start.elapsed();
    println!("Rust Gate Evaluation (50,000 runs): {:.2?} ({:.0} ops/sec)", d_gate, 50000.0 / d_gate.as_secs_f64());
}
