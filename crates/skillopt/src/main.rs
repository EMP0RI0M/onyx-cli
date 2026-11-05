use skillopt::skillopt::datasets::base::{DataLoader, StandardDataLoader};
use skillopt::skillopt::engine::ReflActPipeline;
use skillopt::skillopt::envs::base::{EnvAdapter, MockEnvAdapter};
use skillopt::skillopt::evaluation::gate::{evaluate_gate, GateMetric, GateOptions};
use skillopt::skillopt::optimizer::clip::rank_and_select;
use skillopt::skillopt::optimizer::scheduler::{LRScheduler, SchedulerMode};
use skillopt::skillopt::optimizer::slow_update::{
    has_slow_update_field, inject_empty_slow_update_field, replace_slow_update_field,
};
use skillopt::skillopt::types::{Edit, EditOp, GateAction, Patch, RolloutResult};
use skillopt::skillopt::utils::scoring::{compute_score, skill_hash};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("============================================================");
    println!("      SkillOpt-RS: Autonomous Skill Optimization Engine    ");
    println!("============================================================");

    // 1. Initial State
    let mut current_skill = String::from(
        "# General Task Solver Skill\n\
         ## Step 1: Read problem statement and extract parameters.\n\
         ## Step 2: Formulate solution plan.\n\
         ## Step 3: Emit final answer.\n"
    );
    let mut best_skill = current_skill.clone();
    let mut current_score = 0.50;
    let mut best_score = 0.50;
    let mut best_step = 0;

    let total_steps = 4;
    let mut scheduler = LRScheduler::new(SchedulerMode::Linear, 6, 2, total_steps);
    let env = MockEnvAdapter::new("benchmark_env", vec!["math", "qa"]);
    let loader = StandardDataLoader;

    println!("[Init] Starting skill hash: {}", skill_hash(&current_skill));
    println!("[Init] Initial baseline score: {:.2}", current_score);
    println!("------------------------------------------------------------");

    // 2. Optimization Epoch Loop
    for step in 1..=total_steps {
        let edit_budget = scheduler.step();
        println!("\n>>> Step {}/{} [Edit Budget: {}]", step, total_steps, edit_budget);

        // Stage 1: Batch Rollout
        let train_batch = loader.build_train_batch(3, 42 + step as u64, None);
        let rollout_results = env.rollout(&train_batch, &current_skill, "/tmp").await?;
        let (hard_acc, soft_acc) = compute_score(&rollout_results);
        println!("  [1. Rollout] Processed {} items | hard: {:.2}, soft: {:.2}", rollout_results.len(), hard_acc, soft_acc);

        // Stage 2 & 3: Reflection & Aggregation (Simulate error analysis)
        let raw_patch = Patch {
            edits: vec![
                Edit {
                    op: EditOp::InsertAfter,
                    target: "## Step 1: Read problem statement and extract parameters.".to_string(),
                    content: format!("## Step 1.{}: Verify parameter bounds strictly.", step),
                    support_count: Some(10),
                    ..Default::default()
                },
                Edit {
                    op: EditOp::Replace,
                    target: "## Step 3: Emit final answer.".to_string(),
                    content: format!("## Step 3: Run sanity verification check (v{}).", step),
                    support_count: Some(8),
                    ..Default::default()
                },
                Edit {
                    op: EditOp::Append,
                    content: format!("## Heuristic {}: Prefer fast path when simple.", step),
                    support_count: Some(3),
                    ..Default::default()
                },
            ],
            reasoning: format!("Step {} identified boundary and validation failures", step),
            ranking_details: None,
        };

        // Stage 4: Gradient Clipping / Selection
        let clipped_patch = rank_and_select(&current_skill, raw_patch, edit_budget, None).await?;
        println!("  [2. Gradient Clip] Selected {} most impactful edits from candidate pool", clipped_patch.edits.len());

        // Stage 5: Apply Patch to create candidate skill
        let candidate_skill = ReflActPipeline::apply_patch(&current_skill, &clipped_patch);
        let cand_hard = (0.60 + 0.08 * (step as f64)).min(0.95);
        let cand_soft = (0.65 + 0.07 * (step as f64)).min(0.98);

        // Stage 6: Validation Gate
        let gate_opts = GateOptions {
            cand_soft,
            metric: GateMetric::Mixed,
            mixed_weight: 0.5,
            ..Default::default()
        };
        let gate_result = evaluate_gate(
            &candidate_skill,
            cand_hard,
            &current_skill,
            current_score,
            &best_skill,
            best_score,
            best_step,
            step as i64,
            gate_opts,
        );

        match gate_result.action {
            GateAction::AcceptNewBest => {
                println!("  [3. Gate] Decision: ACCEPT_NEW_BEST (score {:.3} -> {:.3})", current_score, gate_result.current_score);
                current_skill = candidate_skill;
                best_skill = current_skill.clone();
                current_score = gate_result.current_score;
                best_score = gate_result.best_score;
                best_step = gate_result.best_step;
            }
            GateAction::Accept => {
                println!("  [3. Gate] Decision: ACCEPT (score {:.3})", gate_result.current_score);
                current_skill = candidate_skill;
                current_score = gate_result.current_score;
            }
            GateAction::Reject => {
                println!("  [3. Gate] Decision: REJECT (candidate did not improve score)");
            }
        }
    }

    // 3. Epoch Longitudinal Slow Update
    println!("\n------------------------------------------------------------");
    println!(">>> Running Epoch Slow-Update (Longitudinal Regularization)...");
    let skill_with_block = inject_empty_slow_update_field(&best_skill);
    let final_skill = replace_slow_update_field(
        &skill_with_block,
        "Prioritize fast sanity checking and parameter boundary validation.",
    );

    println!("[Epoch Finalized]");
    println!("  - Best Step: {}", best_step);
    println!("  - Best Validation Score: {:.3}", best_score);
    println!("  - Final Skill Hash: {}", skill_hash(&final_skill));
    println!("  - Protected Slow-Update Installed: {}", has_slow_update_field(&final_skill));
    println!("============================================================");

    Ok(())
}
