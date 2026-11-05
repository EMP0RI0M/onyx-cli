// Converted via p2r
"SkillOpt-Sleep — vendored validation gate.

This is a self-contained copy of the SkillOpt validation gate so the sleep
engine has ZERO dependency on the research package (skillopt/*). The research
repo's ``skillopt.evaluation.gate`` is the reference implementation and the two
are kept behaviourally identical; vendoring keeps this open-source tool
decoupled from the paper's experiment code.
";

#[derive(Debug, Clone)]
struct GateResult {
action : String,
current_skill : String,
current_score : f64,
best_skill : String,
best_score : f64,
best_step : isize
}
/*! Project (hard, soft) onto a single comparison metric. */
fn select_gate_score(hard: f64, soft: f64, metric: String, mixed_weight: f64) -> f64 {


if metric == "hard" {
return ((hard) as f64);
;

}
if metric == "soft" {
return ((soft) as f64);
;

}
if metric == "mixed" {
let mut w = max(0, min(1, ((mixed_weight) as f64)));
;
return 1 - w * ((hard) as f64) + w * ((soft) as f64);
;

}
panic!("ValueError(format!("{:?}{:?}{:?}", "unknown gate metric ", metric, "; expected hard/soft/mixed"))")}
/*! Pure gate decision: compare candidate score to current/best. */
fn evaluate_gate(candidate_skill: String, cand_hard: f64, current_skill: String, current_score: f64, best_skill: String, best_score: f64, best_step: isize, global_step: isize) -> GateResult {


let mut cand_score = select_gate_score(cand_hard, cand_soft, metric, mixed_weight);
if cand_score > current_score {
if cand_score > best_score {
return GateResult {
action : "accept_new_best",
current_skill : candidate_skill,
current_score : cand_score,
best_skill : candidate_skill,
best_score : cand_score,
best_step : global_step,
};
;

}
;
return GateResult {
action : "accept",
current_skill : candidate_skill,
current_score : cand_score,
best_skill : best_skill,
best_score : best_score,
best_step : best_step,
};
;

}
return GateResult {
action : "reject",
current_skill : current_skill,
current_score : current_score,
best_skill : best_skill,
best_score : best_score,
best_step : best_step,
};
}

