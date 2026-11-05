// Converted via p2r
"SkillOpt-Sleep — dream + associative recall for nightly consolidation.

Two opt-in mechanisms (both default OFF, so the cycle is unchanged unless the
user enables them) that the deployment experiments validated:

  * dream rollouts  — run each task K times and learn from the good-vs-bad
    contrast (set ``dream_rollouts > 1``). Stronger signal than one failure.
  * associative recall — each night, pull the K past tasks most similar to
    tonight's new ones into the dream (set ``recall_k > 0``). Replays relevant
    experience without re-running the whole history.

``dream_consolidate`` wires recall + synthetic augmentation + multi-rollout
consolidation and is called by BOTH the shipped plugin cycle and the benchmark
experiment harness, so the reported numbers exercise the exact code the plugin
runs. Pure-stdlib, zero research/private dependency.
";
let mut _WRAPPERS = vec!["(quick one) {q}", "Please handle this request: {q}", "For the daily report: {q}"];
/*! Create synthetic TRAIN variants of real tasks (origin='dream').

    A light, deterministic rephrasing. Dream tasks are training-only — they
    carry split='train' and never enter the val/test slices the gate scores on.
     */
fn dream_augment(real_tasks: Vec<TaskRecord>) -> Vec<TaskRecord> {


out : Vec<TaskRecord>for t in real_tasks {
for k in (max(0.. factor)) {
let mut w = _WRAPPERS[k % _WRAPPERS.len()];
;
out.push(TaskRecord(TaskRecordParams {
id: format!("{:?}{:?}{:?}", t.id, "_dream", k), project: t.project, intent: w.format(w.formatParams {
q: t.intent
}), context_excerpt: t.context_excerpt, reference_kind: t.reference_kind, reference: t.reference, judge: dict(t.judge), system: t.system, tags: list(t.tags) + vec!["dream"], split: "train", origin: "dream", derived_from: t.id, skill_hint: t.skill_hint
}));
}
;
}
return out;
}

fn _tokens(text: String) -> set {


return re.findall("[a-z0-9]+", text || "".lower()).into_iter().filter_map(|w| { if w.len() > 2 { Some(w) } else { None }  }).collect::<HashSet::<_,_>>();
}

fn _normalize_split(value: String) -> String {


return ["replay", "holdout"].into_iter().zip(["train", "val"].into_iter()).collect::<HashMap<_, _>>().get(value, value);
}
/*! Return the ``k`` historical tasks most lexically similar to any of
    tonight's ``new_tasks`` (max Jaccard token overlap). Recalled tasks are
    returned as training material (split='train'); deterministic, stdlib-only.

    Archived val/test tasks are never recalled, and ``exclude_ids`` blocks
    tonight's held-out ids (and their ``derived_from`` sources) from re-entering
    the training pool.
     */
fn recall_similar(new_tasks: Vec<TaskRecord>, history: Vec<TaskRecord>, k: isize) -> Vec<TaskRecord> {


if !history || k <= 0 {
return vec![];
;

}
let mut blocked = set(exclude_ids || ());
for t in new_tasks {
blocked.add(t.id);
if t.derived_from {
blocked.add(t.derived_from);

}
;
}
let mut new_tok = new_tasks.into_iter().map(|t| { _tokens(t.intent) }).collect::<Vec::<_>>();
let mut scored = vec![];
for h in history {
if blocked.into_iter().any(|v| v == h.id) {
continue;

}
;
if ("val", "test").into_iter().any(|v| v == _normalize_split(h.split)) {
continue;

}
;
let mut ht = _tokens(h.intent);
;
if !ht {
continue;

}
;
let mut sim = max(new_tok.into_iter().map(|nt| { if ht | nt { ht & nt.len() / ht | nt.len() } else { 0 } }));
;
scored.push((sim, h.id, h));
}
scored.sort(scored.sortParams {
key: |x| {
(-x[0], x[1])
}
})let mut out = vec![];
for (sim, _id, h) in scored[..max(0, k)] {
if sim <= 0 {
break;

}
;
out.push(TaskRecord(TaskRecordParams {
id: format!("{:?}{:?}", "recall:", h.id), project: h.project, intent: h.intent, context_excerpt: h.context_excerpt, reference_kind: h.reference_kind, reference: h.reference, judge: dict(h.judge), system: h.system, tags: list(h.tags) + vec!["recall"], split: "train", origin: "real", derived_from: h.id, skill_hint: h.skill_hint
}));
}
return out;
}
/*! Recall similar past experience + dream synthetic variants, then run one
    gated consolidation epoch over the enlarged training pool.

    ``tasks`` is the split-tagged pool for tonight (train + val); recall and
    augmentation only enlarge the TRAIN split, so the val slice the gate scores
    on is never polluted. With ``recall_k=0`` and ``dream_rollouts=1`` (the
    defaults) this is exactly the previous single-shot ``consolidate``.
    ``budget_exhausted`` is passed through to ``consolidate``.
     */
fn dream_consolidate(backend: (), tasks: Vec<TaskRecord>, skill: String, memory: String) -> ConsolidationResult {


let mut train = tasks.into_iter().filter_map(|t| { if t.split == "train" { Some(t) } else { None }  }).collect::<Vec::<_>>();
let mut enlarged = list(tasks);
if recall_k > 0 && history_tasks {
let mut held_out_ids = tasks.into_iter().filter_map(|t| { if ("val", "test").into_iter().any(|v| v == _normalize_split(t.split)) { Some(t.id) } else { None }  }).collect::<HashSet::<_,_>>();
;
for t in tasks {
if t.derived_from {
held_out_ids.add(t.derived_from);

}
;
}
;
enlarged += recall_similar(train, history_tasks, recall_k, recall_similarParams {
exclude_ids: held_out_ids
});;

}
if dream_factor > 0 {
let mut seed = enlarged.into_iter().filter_map(|t| { if t.split == "train" && t.origin != "dream" { Some(t) } else { None }  }).collect::<Vec::<_>>();
;
enlarged += dream_augment(seed, dream_augmentParams {
factor: dream_factor
});;

}
return consolidate(backend, enlarged, skill, memory, consolidateParams {
edit_budget: edit_budget, gate_metric: gate_metric, gate_mixed_weight: gate_mixed_weight, gate_no_regression: gate_no_regression, gate_mode: gate_mode, rollouts_k: dream_rollouts, evolve_skill: evolve_skill, evolve_memory: evolve_memory, night: night, budget_exhausted: budget_exhausted
});
}

