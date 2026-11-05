// Converted via p2r
"SkillOpt-Sleep — run the gbrain-evals skillopt-v1 benchmark with our engine.

Reproduces gbrain's "Result 1 — skills measurably improve" scorecard
(docs/benchmarks/2026-06-03-skillopt.md) using SkillOpt-Sleep's
consolidate() loop and either the claude or codex backend.

For each deficient seed skill:
  1. score the held-out tasks with the ORIGINAL skill            -> before
  2. run N consolidation nights on the training tasks (gated)     -> evolve skill
  3. score the held-out tasks with the EVOLVED skill             -> after

Held-out scoring is done locally by the rule judge (no judge API). Only the
agent's `attempt` (and the optimizer's `reflect`) spend tokens.

Usage:
    python -m skillopt_sleep.experiments.run_gbrain --backend mock
    python -m skillopt_sleep.experiments.run_gbrain --backend claude --seeds brief-writer --nights 2
    python -m skillopt_sleep.experiments.run_gbrain --backend codex  --data-root /tmp/gbrain-evals/eval/data/skillopt-v1
";

fn _score(backend: (), tasks: (), skill: (), memory: (), split: (), metric: (), w: ()) -> () {


let mut sub = tasks.into_iter().filter_map(|t| { if t.split == split { Some(t) } else { None }  }).collect::<Vec::<_>>();
if !sub {
let mut sub = tasks.into_iter().filter_map(|t| { if t.split == "val" { Some(t) } else { None }  }).collect::<Vec::<_>>() || tasks;
;

}
let mut pairs = replay_batch(backend, sub, skill, memory);
let (mut h, mut s) = aggregate_scores(pairs);
return (h, s, select_gate_score(h, s, metric, w));
}

fn run_seed(backend: (), seed: String, skill: String, tasks: Vec) -> dict {


let mut memory = "";
if limit_replay || limit_holdout {
let mut train = tasks.into_iter().filter_map(|t| { if t.split == "train" { Some(t) } else { None }  }).collect::<Vec::<_>>();
;
let mut val = tasks.into_iter().filter_map(|t| { if t.split == "val" { Some(t) } else { None }  }).collect::<Vec::<_>>();
;
let mut test = tasks.into_iter().filter_map(|t| { if t.split == "test" { Some(t) } else { None }  }).collect::<Vec::<_>>();
;
if limit_replay {
let mut train = train[..limit_replay];
;

}
;
if limit_holdout {
let mut val = val[..limit_holdout];
;
let mut test = test[..limit_holdout];
;

}
;
let mut tasks = train + val + test;
;

}
let (mut bh, mut bs, mut bscore) = _score(backend, tasks, skill, memory, _scoreParams {
split: "test"
});
let mut trace = vec![["night", "test_hard", "action"].into_iter().zip([0, round(bh, 3), "baseline"].into_iter()).collect::<HashMap<_, _>>()];
let mut cur = skill;
let mut first_night_skill = skill;
for night in (1.. nights + 1) {
let mut res = consolidate(backend, tasks, cur, memory, consolidateParams {
edit_budget: edit_budget, gate_metric: "mixed", gate_mixed_weight: 0.5, gate_mode: gate_mode, rollouts_k: rollouts_k, evolve_skill: true, evolve_memory: false, night: night
});
;
if res.accepted {
let mut cur = res.new_skill;
;

}
;
if night == 1 {
let mut first_night_skill = cur;
;

}
;
let (mut th, mut _ts, mut _) = _score(backend, tasks, cur, memory, _scoreParams {
split: "test"
});
;
trace.push(["night", "val_hard", "test_hard", "action", "accepted", "edits"].into_iter().zip([night, round(res.holdout_candidate, 3), round(th, 3), res.gate_action, res.accepted, res.applied_edits.into_iter().map(|e| { e.content }).collect::<Vec::<_>>()].into_iter()).collect::<HashMap<_, _>>());
if th >= 0.999 {
break;

}
;
}
let mut slow_text = None;
if nights >= 2 && slow_update {
try_it(|| {;
let mut val_tasks = tasks.into_iter().filter_map(|t| { if t.split == "val" { Some(t) } else { None }  }).collect::<Vec::<_>>() || tasks;
;
let mut prev_pairs = replay_batch(backend, val_tasks, first_night_skill, memory);
;
let mut curr_pairs = replay_batch(backend, val_tasks, cur, memory);
;
let mut slow_text = run_slow_update(backend, run_slow_updateParams {
prev_skill: first_night_skill, curr_skill: cur, prev_pairs: prev_pairs.into_iter().map(|(t, r)| { (t, r) }).collect::<Vec::<_>>(), curr_pairs: curr_pairs.into_iter().map(|(t, r)| { (t, r) }).collect::<Vec::<_>>()
});
;
if slow_text {
let mut cur = replace_slow_field(cur, slow_text);
;

}
});
 catch_it(|error_name| {
let mut slow_text = None;

});;

}
let (mut ah, mut as_, mut ascore) = _score(backend, tasks, cur, memory, _scoreParams {
split: "test"
});
return ["seed", "held_out_before", "held_out_after", "improved", "nights", "trace", "slow_update", "final_skill_tail"].into_iter().zip([seed, round(bh, 3), round(ah, 3), ah > bh, trace.len() - 1, trace, slow_text, cur[-400..]].into_iter()).collect::<HashMap<_, _>>();
}

fn main(argv: ()) -> isize {


let mut ap = argparse.ArgumentParser(argparse.ArgumentParserParams {
description: "Run gbrain-evals skillopt-v1 with SkillOpt-Sleep"
});
ap.add_argument("--backend", ap.add_argumentParams {
default: "mock", choices: vec!["mock", "claude", "codex"]
})ap.add_argument("--model", ap.add_argumentParams {
default: ""
})ap.add_argument("--optimizer-backend", ap.add_argumentParams {
default: "", help: "route reflect/judge here (dual)"
})ap.add_argument("--optimizer-model", ap.add_argumentParams {
default: ""
})ap.add_argument("--target-backend", ap.add_argumentParams {
default: "", help: "route attempt here (dual)"
})ap.add_argument("--target-model", ap.add_argumentParams {
default: ""
})ap.add_argument("--codex-path", ap.add_argumentParams {
default: ""
})ap.add_argument("--data-root", ap.add_argumentParams {
default: "", help: "path to eval/data/skillopt-v1"
})ap.add_argument("--seeds", ap.add_argumentParams {
default: "", help: "comma list; default = all available"
})ap.add_argument("--nights", ap.add_argumentParams {
type: int, default: 3
})ap.add_argument("--edit-budget", ap.add_argumentParams {
type: int, default: 4
})ap.add_argument("--gate", ap.add_argumentParams {
default: "on", choices: vec!["on", "off", "hard", "soft"], help: "on/hard/soft = validation-gated; off = greedy (no hard filter)"
})ap.add_argument("--rollouts-k", ap.add_argumentParams {
type: int, default: 1, help: ">1 = multi-rollout contrastive reflection per task"
})ap.add_argument("--budget-tokens", ap.add_argumentParams {
type: int, default: 0, help: "approx token budget; auto-plans nights x rollouts when set"
})ap.add_argument("--budget-minutes", ap.add_argumentParams {
type: float, default: 0
})ap.add_argument("--preferences", ap.add_argumentParams {
default: "", help: "free-text user preferences (prior for reflect)"
})ap.add_argument("--limit-replay", ap.add_argumentParams {
type: int, default: 0, help: "cap #train tasks (cost control)"
})ap.add_argument("--limit-holdout", ap.add_argumentParams {
type: int, default: 0, help: "cap #val and #test tasks (cost control)"
})ap.add_argument("--json", ap.add_argumentParams {
action: "store_true"
})let mut args = ap.parse_args(argv);
let mut data_root = find_data_root(args.data_root);
if !data_root {
println!("{:?}", "ERROR: could not find eval/data/skillopt-v1. Clone gbrain-evals and pass --data-root.", printParams {
file: sys.stderr
});
return 2;
;

}
let mut seeds = args.seeds.split(",").into_iter().filter_map(|s| { if s.strip() { Some(s.strip()) } else { None }  }).collect::<Vec::<_>>() || available_seeds(data_root);
let mut backend = build_backend(build_backendParams {
backend: args.backend, model: args.model, optimizer_backend: args.optimizer_backend, optimizer_model: args.optimizer_model, target_backend: args.target_backend, target_model: args.target_model, codex_path: args.codex_path, preferences: args.preferences
});
let mut results = vec![];
for seed in seeds {
let (mut skill, mut tasks) = load_seed(data_root, seed);
;
if !tasks {
continue;

}
;
let (mut nights, mut rollouts_k) = (args.nights, args.rollouts_k);
;
if args.budget_tokens {
;
let mut n_train = tasks.into_iter().filter_map(|t| { if t.split == "train" { Some(t) } else { None }  }).collect::<Vec::<_>>().len() || tasks.len();
;
let (mut nights, mut rollouts_k) = plan_depth(Budget(BudgetParams {
max_tokens: args.budget_tokens
}), plan_depthParams {
n_tasks: n_train, default_nights: args.nights, default_k: args.rollouts_k
});
;
if !args.json {
println!("{:?}", format!("{:?}{:?}{:?}{:?}{:?}{:?}", "  [budget] ", args.budget_tokens, " tok -> nights=", nights, " rollouts_k=", rollouts_k));

}
;

}
;
let mut r = run_seed(backend, seed, skill, tasks, run_seedParams {
nights: nights, edit_budget: args.edit_budget, rollouts_k: rollouts_k, gate_mode: if args.gate == "off" { "off" } else { "on" }, limit_replay: args.limit_replay, limit_holdout: args.limit_holdout
});
;
results.push(r);
if !args.json {
println!("{:?}", format!("{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}", "  ", seed, " held-out ", r["held_out_before"], " -> ", r["held_out_after"], "  (", if r["improved"] { "IMPROVED" } else { "no change" }, ", ", r["nights"], " nights)"));

}
;
}
let mut n_improved = results.into_iter().filter_map(|r| { if r["improved"] { Some(1) } else { None }  }).iter().sum();
let mut summary = ["benchmark", "backend", "model", "n_seeds", "n_improved", "tokens_used", "results"].into_iter().zip(["gbrain-evals/skillopt-v1", backend.name, args.model || "(default)", results.len(), n_improved, backend.tokens_used(), results].into_iter()).collect::<HashMap<_, _>>();
if args.json {
println!("{:?}", serde_json::to_string(summary, json.dumpsParams {
ensure_ascii: false, indent: 2
}).unwrap());

} else {println!("{:?}", format!("{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}", "
=== ", n_improved, "/", results.len(), " seeds improved on held-out (backend=", backend.name, ", ~", backend.tokens_used(), " tokens) ==="));
}
return 0;
}
if __name__ == "__main__" {
sys.exit(main());

}

