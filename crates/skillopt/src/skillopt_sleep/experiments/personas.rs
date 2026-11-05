// Converted via p2r
"SkillOpt-Sleep — persona task fixtures for the validation experiment.

Each persona is a list of TaskRecords with EXACT checkable references and a
`rule:<key>` tag naming the single skill rule that makes the task solvable
(consumed by MockBackend). This lets the experiment prove — deterministically,
with no API — that nightly consolidation lifts a held-out score and that the
gate blocks regressions.

Personas mirror the user's framing: programmer / researcher / analyst.
";

fn _t(i: (), intent: (), ref: (), rule: (), project: (), outcome: ()) -> TaskRecord {


return TaskRecord(TaskRecordParams {
id: format!("{:?}{:?}{:?}{:?}", "persona_", rule, "_", i), project: project, intent: intent, context_excerpt: "", attempted_solution: "", outcome: outcome, reference_kind: "exact", reference: ref, tags: vec![format!("{:?}{:?}", "rule:", rule)], source_sessions: vec![format!("{:?}{:?}", "sess_", i)]
});
}
/*! Researcher who always wants arXiv ids wrapped in <answer> tags. */
fn researcher_persona() -> Vec<TaskRecord> {


let mut items = vec![("Give me the arXiv id for the SkillOpt paper", "arXiv:2605.23904"), ("What's the arXiv id of the Attention paper?", "arXiv:1706.03762"), ("arXiv id for the GAN paper?", "arXiv:1406.2661"), ("arXiv id for BERT?", "arXiv:1810.04805"), ("arXiv id for the ResNet paper?", "arXiv:1512.03385"), ("arXiv id for the Adam optimizer paper?", "arXiv:1412.6980"), ("arXiv id for Dropout?", "arXiv:1207.0580"), ("arXiv id for the Transformer-XL paper?", "arXiv:1901.02860"), ("arXiv id for word2vec?", "arXiv:1301.3781"), ("arXiv id for the VAE paper?", "arXiv:1312.6114"), ("arXiv id for batch norm?", "arXiv:1502.03167"), ("arXiv id for GPT-3?", "arXiv:2005.14165")];
out : Vec<TaskRecord>for (i, (q, a)) in items.iter().enumerate() {
let mut t = _t(i, q, a, "wrap-answer");
;
let mut t.tags = vec!["rule:wrap-answer", "rule:arxiv-id"];
;
out.push(t);
}
return out;
}
/*! Programmer who wants imperative-mood commit subjects. */
fn programmer_persona() -> Vec<TaskRecord> {


let mut items = vec![("commit message for adding a login form", "Add login form"), ("commit message for fixing the null pointer bug", "Fix null pointer in parser"), ("commit message for updating the README", "Update README"), ("commit message for removing dead code", "Remove dead code"), ("commit message for bumping the version", "Bump version to 1.2.0"), ("commit message for refactoring the auth module", "Refactor auth module"), ("commit message for adding tests", "Add unit tests for scheduler"), ("commit message for fixing the CI pipeline", "Fix CI pipeline")];
return items.iter().enumerate().into_iter().map(|(i, (q, a))| { _t(i, q, a, "commit-imperative") }).collect::<Vec::<_>>();
}
/*! A task whose 'fix' is a known-bad rule; used to prove the gate rejects
    regressions. The MockBackend proposes the harmful rule on this failure,
    but applying it does NOT raise the held-out score, so the gate must reject.
     */
fn harmful_edit_task() -> TaskRecord {


let mut t = _t(99, "answer this freely", "THIS_WILL_NOT_MATCH", "__harmful__");
let mut t.reference = "an-answer-that-the-harmful-rule-cannot-produce";
return t;
}
let mut PERSONAS = ["researcher", "programmer"].into_iter().zip([researcher_persona, programmer_persona].into_iter()).collect::<HashMap<_, _>>();

