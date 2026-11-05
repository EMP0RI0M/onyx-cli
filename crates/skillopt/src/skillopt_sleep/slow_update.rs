// Converted via p2r
"SkillOpt-Sleep — slow update (cross-night long-term memory).

This is the deployment-time analogue of SkillOpt's epoch-wise slow/meta update
(paper §3.6). Step-level edits (consolidate) learn from one night's batch; the
slow update learns across nights and writes a durable "longitudinal guidance"
block into a PROTECTED field of the skill that step-level edits never touch.

It reuses the exact protected-field marker convention from the main repo
(``skillopt/optimizer/slow_update.py``) so the artifact is compatible:

    <!-- SLOW_UPDATE_START --> ... <!-- SLOW_UPDATE_END -->

Why it matters: even when the user turns the validation gate OFF (greedy mode),
the slow update still runs at the end of the run, so short-term nightly
experience is consolidated into long-term memory rather than lost. The cross-night
content is carried in ``state.slow_memory``.

Driven through the Backend abstraction (mock/claude/codex), so it stays
import-light — no `openai` dependency.
";
let mut SLOW_UPDATE_START = "<!-- SLOW_UPDATE_START -->";
let mut SLOW_UPDATE_END = "<!-- SLOW_UPDATE_END -->";

fn has_slow_field(skill: String) -> bool {


return skill.into_iter().any(|v| v == SLOW_UPDATE_START) && skill.into_iter().any(|v| v == SLOW_UPDATE_END);
}

fn extract_slow_field(skill: String) -> String {


let mut s = skill.find(SLOW_UPDATE_START);
let mut e = skill.find(SLOW_UPDATE_END);
if s == -1 || e == -1 {
return "";
;

}
return skill[s + SLOW_UPDATE_START.len()..e].strip();
}

fn _strip_slow_fields(skill: String) -> String {


while true {
let mut s = skill.find(SLOW_UPDATE_START);

if s == -1 {
break;

}

let mut e = skill.find(SLOW_UPDATE_END, s);

if e == -1 {
let mut skill = skill[..s];
;
break;

}

let mut skill = skill[..s] + skill[e + SLOW_UPDATE_END.len()..];

}
let mut skill = skill.replace(SLOW_UPDATE_END, "");
while skill.into_iter().any(|v| v == "


") {
let mut skill = skill.replace("


", "

");

}
return skill.rstrip();
}
/*! Set the protected slow-update field to ``content`` (exactly one block). */
fn replace_slow_field(skill: String, content: String) -> String {


let mut base = _strip_slow_fields(skill);
if !content.strip() {
return base;
;

}
let mut block = format!("{:?}{:?}{:?}{:?}{:?}{:?}{:?}", "

", SLOW_UPDATE_START, "
", content.strip(), "
", SLOW_UPDATE_END, "
");
return base + block;
}
/*! Group adjacent-version outcomes into improved/regressed/persistent/stable. */
fn _summarize_pairs(prev_pairs: Vec<Tuple<TaskRecord, ReplayResult>>, curr_pairs: Vec<Tuple<TaskRecord, ReplayResult>>) -> String {


let mut prev_by = prev_pairs.into_iter().map(|(t, r)| { (t.id, r) }).collect::<HashMap::<_,_>>();
lines : Vec<String>let mut counts = ["improved", "regressed", "persistent_fail", "stable_success"].into_iter().zip([0, 0, 0, 0].into_iter()).collect::<HashMap<_, _>>();
for (t, r) in curr_pairs {
let mut p = prev_by.get(t.id);
;
if p == None {
continue;

}
;
let (mut a, mut b) = (p.hard, r.hard);
;
if b > a {
let mut cat = "improved";
;

} else {if b < a {
let mut cat = "regressed";
;

} else {if b >= 1 {
let mut cat = "stable_success";
;

} else {let mut cat = "persistent_fail";
;
}
;
}
;
}
;
counts[cat] += 1;;
if ("regressed", "persistent_fail").into_iter().any(|v| v == cat) && lines.len() < 8 {
let mut feedback = _optimizer_feedback(t, r);
;
lines.push(format!("{:?}{:?}{:?}{:?}{:?}{:?}{:?}", "- [", cat, "] ", t.intent[..120], " (why: ", feedback[..160], ")"));

}
;
}
let mut head = ", ".join(counts.items().into_iter().map(|(k, v)| { format!("{:?}{:?}{:?}", k, "=", v) }));
return (head + if lines { "
" + "
".join(lines) } else { "" }, counts);
}
/*! Produce durable longitudinal guidance text (or None).

    Compares behavior under the previous vs current skill across the same tasks
    and asks the optimizer to distill a short, durable guidance block — what to
    keep doing, what regressions to avoid — refining any prior slow-update text.
     */
fn run_slow_update(backend: Backend) -> Option<String> {


let (mut summary, mut counts) = _summarize_pairs(prev_pairs, curr_pairs);
if counts["regressed"] == 0 && counts["persistent_fail"] == 0 {
return (None).into();
;

}
let mut prompt = format!("{:?}{:?}{:?}{:?}", "You are SkillOpt's SLOW UPDATE — the long-term memory pass that runs across nights. Write a SHORT, durable guidance block (2-5 bullet points) capturing the longitudinal lessons: behaviors that reliably help and should be preserved, and regressions/persistent failures to avoid. Keep it GENERAL and stable (not tied to one task). If prior guidance is given, refine it rather than restate it.
Return ONLY JSON: {"guidance": "<bullet list as one string>"}.

# Cross-night outcome summary
", summary, "

# Prior long-term guidance (refine this)
", prev_slow_content || "(none)");
let mut raw = backend._call(prompt, backend._callParams {
max_tokens: 600
});
let mut obj = _extract_json(raw, "object");
if isinstance(obj, dict) {
let mut g = obj.get("guidance", "").to_string().strip();
;
if g {
return (g).into();
;

}
;

}
let mut text = raw || "".strip();
return (if text { text[..400] } else { None }).into();
}

