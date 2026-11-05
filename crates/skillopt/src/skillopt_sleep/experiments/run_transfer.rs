// Converted via p2r
"SkillOpt-Sleep — skill-transfer experiment (sleep scenario).

Answers: "if I optimize a skill while the agent sleeps using a CHEAP model,
does the learned skill still help an EXPENSIVE model at deploy time?" — and the
reverse. This is the SkillOpt paper's cross-model transfer result, reproduced
in the sleep setting, and it is the core price-difference value proposition:
spend cheap tokens overnight, deploy the frozen skill anywhere.

Protocol, per gbrain seed:
  1. baseline_target = held-out score of the DEFICIENT skill, run on TARGET model
  2. optimize the skill for N nights using the SOURCE model (attempt+reflect)
  3. transferred = held-out score of the LEARNED skill, run on TARGET model,
     with NO further optimization
  4. (reference) direct = held-out score of a skill optimized AND run on TARGET

Report baseline / direct / transferred, mirroring SkillOpt Table "transfer".

Usage:
  python -m skillopt_sleep.experiments.run_transfer      --source-backend claude --source-model haiku      --target-backend claude --target-model sonnet      --seeds brief-writer --nights 2
";

fn _holdout_hard(backend: (), tasks: (), skill: (), memory: ()) -> f64 {


let mut ho = tasks.into_iter().filter_map(|t| { if t.split == "test" { Some(t) } else { None }  }).collect::<Vec::<_>>();
if !ho {
let mut ho = tasks.into_iter().filter_map(|t| { if ("val", "holdout").into_iter().any(|v| v == t.split) { Some(t) } else { None }  }).collect::<Vec::<_>>() || tasks;
;

}
let mut pairs = replay_batch(backend, ho, skill, memory);
let (mut h, mut _s) = aggregate_scores(pairs);
return h;
}

fn _optimize(backend: (), skill: (), tasks: ()) -> String {


let mut cur = skill;
for night in (1.. nights + 1) {
let mut res = consolidate(backend, tasks, cur, "", consolidateParams {
edit_budget: edit_budget, gate_metric: "mixed", evolve_skill: true, evolve_memory: false, night: night
});
;
if res.accepted {
let mut cur = res.new_skill;
;

}
;
if res.holdout_candidate >= 0.999 {
break;

}
;
}
return cur;
}

fn run_seed(seed: (), skill: (), tasks: ()) -> dict {


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
let mut baseline_target = _holdout_hard(target, tasks, skill);
let mut learned_on_source = _optimize(source, skill, tasks, _optimizeParams {
nights: nights, edit_budget: edit_budget
});
let mut transferred = _holdout_hard(target, tasks, learned_on_source);
let mut direct = None;
if do_direct {
let mut learned_on_target = _optimize(target, skill, tasks, _optimizeParams {
nights: nights, edit_budget: edit_budget
});
;
let mut direct = _holdout_hard(target, tasks, learned_on_target);
;

}
return ["seed", "baseline_target", "direct_target", "transferred", "transfer_gain", "learned_skill_tail"].into_iter().zip([seed, round(baseline_target, 3), if direct != None { round(direct, 3) } else { None }, round(transferred, 3), round(transferred - baseline_target, 3), learned_on_source[-300..]].into_iter()).collect::<HashMap<_, _>>();
}

fn main(argv: ()) -> isize {


let mut ap = argparse.ArgumentParser(argparse.ArgumentParserParams {
description: "SkillOpt-Sleep cross-model transfer"
});
ap.add_argument("--source-backend", ap.add_argumentParams {
default: "claude"
})ap.add_argument("--source-model", ap.add_argumentParams {
default: "haiku"
})ap.add_argument("--target-backend", ap.add_argumentParams {
default: "claude"
})ap.add_argument("--target-model", ap.add_argumentParams {
default: "sonnet"
})ap.add_argument("--codex-path", ap.add_argumentParams {
default: ""
})ap.add_argument("--data-root", ap.add_argumentParams {
default: ""
})ap.add_argument("--seeds", ap.add_argumentParams {
default: "brief-writer"
})ap.add_argument("--nights", ap.add_argumentParams {
type: int, default: 2
})ap.add_argument("--edit-budget", ap.add_argumentParams {
type: int, default: 4
})ap.add_argument("--limit-replay", ap.add_argumentParams {
type: int, default: 3
})ap.add_argument("--limit-holdout", ap.add_argumentParams {
type: int, default: 3
})ap.add_argument("--no-direct", ap.add_argumentParams {
action: "store_true", help: "skip the direct reference (saves cost)"
})ap.add_argument("--json", ap.add_argumentParams {
action: "store_true"
})let mut args = ap.parse_args(argv);
let mut data_root = find_data_root(args.data_root);
if !data_root {
println!("{:?}", "ERROR: gbrain-evals skillopt-v1 data not found; pass --data-root", printParams {
file: sys.stderr
});
return 2;
;

}
let mut source = get_backend(args.source_backend, get_backendParams {
model: args.source_model, codex_path: args.codex_path
});
let mut target = get_backend(args.target_backend, get_backendParams {
model: args.target_model, codex_path: args.codex_path
});
let mut seeds = args.seeds.split(",").into_iter().filter_map(|s| { if s.strip() { Some(s.strip()) } else { None }  }).collect::<Vec::<_>>() || available_seeds(data_root);
let mut results = vec![];
for seed in seeds {
let (mut skill, mut tasks) = load_seed(data_root, seed);
;
if !tasks {
continue;

}
;
let mut r = run_seed(seed, skill, tasks, run_seedParams {
source: source, target: target, nights: args.nights, edit_budget: args.edit_budget, limit_replay: args.limit_replay, limit_holdout: args.limit_holdout, do_direct: !args.no_direct
});
;
results.push(r);
if !args.json {
let mut d = if r["direct_target"] != None { format!("{:?}{:?}", " direct=", r["direct_target"]) } else { "" };
;
println!("{:?}", format!("{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}", "  ", seed, " baseline=", r["baseline_target"], " transferred=", r["transferred"], d, "  (gain ", r["transfer_gain"], ")"));

}
;
}
let mut summary = ["experiment", "source", "target", "tokens_source", "tokens_target", "results"].into_iter().zip(["skillopt-sleep/transfer", format!("{:?}{:?}{:?}", args.source_backend, ":", args.source_model), format!("{:?}{:?}{:?}", args.target_backend, ":", args.target_model), source.tokens_used(), target.tokens_used(), results].into_iter()).collect::<HashMap<_, _>>();
if args.json {
println!("{:?}", serde_json::to_string(summary, json.dumpsParams {
ensure_ascii: false, indent: 2
}).unwrap());

} else {println!("{:?}", format!("{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}", "
=== transfer ", summary["source"], " -> ", summary["target"], ": ", results.into_iter().filter_map(|r| { if r["transfer_gain"] > 0 { Some(1) } else { None }  }).iter().sum(), "/", results.len(), " positive ==="));
}
return 0;
}
if __name__ == "__main__" {
sys.exit(main());

}

