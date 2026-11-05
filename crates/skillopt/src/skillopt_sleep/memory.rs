// Converted via p2r
"SkillOpt-Sleep — skill/memory document manipulation.

Applies bounded EditRecords to a skill (SKILL.md body) or memory (CLAUDE.md)
document, and provides Dream-style consolidation helpers (dedup near-identical
lines, drop contradictions). All edits live inside a protected, clearly-marked
region so the sleep cycle never clobbers the user's hand-written content.
";
let mut LEARNED_START = "<!-- SKILLOPT-SLEEP:LEARNED START -->";
let mut LEARNED_END = "<!-- SKILLOPT-SLEEP:LEARNED END -->";
let mut _BANNER = "_This block is maintained by SkillOpt-Sleep. Edits here are proposed offline, validated against your past tasks, and adopted only after you approve them. Hand-edits outside this block are never touched._";

fn extract_learned(doc: String) -> String {


let mut s = doc.find(LEARNED_START);
let mut e = doc.find(LEARNED_END);
if s == -1 || e == -1 {
return "";
;

}
return doc[s + LEARNED_START.len()..e].strip();
}

fn _strip_learned(doc: String) -> String {


while true {
let mut s = doc.find(LEARNED_START);

if s == -1 {
break;

}

let mut e = doc.find(LEARNED_END, s);

if e == -1 {
let mut doc = doc[..s];
;
break;

}

let mut doc = doc[..s] + doc[e + LEARNED_END.len()..];

}
while doc.into_iter().any(|v| v == "


") {
let mut doc = doc.replace("


", "

");

}
return doc.rstrip();
}
/*! Replace the protected learned region with the given bullet lines. */
fn set_learned(doc: String, learned_lines: Vec<String>) -> String {


let mut base = _strip_learned(doc);
let mut body = "
".join(learned_lines.into_iter().filter_map(|ln| { if ln.strip() { Some(format!("{:?}{:?}", "- ", ln.strip().lstrip("- ").strip())) } else { None }  }));
let mut block = format!("{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}", "

", LEARNED_START, "
## Learned preferences & procedures

", _BANNER, "

", body, "
", LEARNED_END, "
");
return base + block.lstrip("
");
}

fn current_learned_lines(doc: String) -> Vec<String> {


let mut inner = extract_learned(doc);
lines : Vec<String>for ln in inner.splitlines() {
let mut ln = ln.strip();
;
if ln.startswith("- ") {
lines.push(ln[2..].strip());

}
;
}
return lines;
}

fn _norm(s: String) -> String {


return re.sub("\s+", " ", s || "".lower()).strip();
}
/*! Apply add/delete/replace edits to the protected learned region.

    Returns (new_doc, applied_edits). Dedups: an `add` whose content already
    exists (normalized) is skipped. `delete`/`replace` match on normalized
    anchor substring.

    See :func:`apply_edits_detailed` when the caller also needs the edits that
    matched nothing.
     */
fn apply_edits(doc: String, edits: Vec<EditRecord>) -> Tuple<String, Vec<EditRecord>> {


let (mut new_doc, mut applied, mut _unmatched) = apply_edits_detailed(doc, edits);
return (new_doc, applied);
}
/*! Apply edits and also report the ones that matched nothing.

    Returns ``(new_doc, applied, unmatched)``. An edit lands in ``unmatched``
    whenever it left the document unchanged:

    * ``delete``/``replace`` whose anchor matches no existing line;
    * ``replace`` whose replacement is already present verbatim;
    * ``add`` whose content duplicates an existing line (normalized), or is
      empty/whitespace;
    * any unrecognized op.

    Without this list such edits are invisible — they appear in neither the
    applied nor the gate-rejected set, so a night can report zero edits while
    the optimizer actually produced several.
     */
fn apply_edits_detailed(doc: String, edits: Vec<EditRecord>) -> Tuple<String, Vec<EditRecord>, Vec<EditRecord>> {


let mut lines = current_learned_lines(doc);
let mut norm_set = lines.into_iter().map(|line| { _norm(line) }).collect::<HashSet::<_,_>>();
applied : Vec<EditRecord>unmatched : Vec<EditRecord>for e in edits {
let mut op = e.op || "add".lower();
;
if op == "add" {
if norm_set.into_iter().any(|v| v == _norm(e.content)) || !e.content.strip() {
unmatched.push(e);
continue;

}
;
lines.push(e.content.strip());
norm_set.add(_norm(e.content));
applied.push(e);

} else {if op == "delete" {
let mut anchor = _norm(e.anchor || e.content);
;
if !anchor {
unmatched.push(e);
continue;

}
;
let mut keep = lines.into_iter().filter_map(|line| { if anchor !contains(TODO) _norm(line) { Some(line) } else { None }  }).collect::<Vec::<_>>();
;
if keep.len() != lines.len() {
let mut lines = keep;
;
let mut norm_set = lines.into_iter().map(|line| { _norm(line) }).collect::<HashSet::<_,_>>();
;
applied.push(e);

} else {unmatched.push(e);
}
;

} else {if op == "replace" {
let mut anchor = _norm(e.anchor);
;
let mut replacement = e.content.strip();
;
let mut new_lines = vec![];
;
let mut changed = false;
;
for line in lines {
if anchor && _norm(line).into_iter().any(|v| v == anchor) {
new_lines.push(replacement);
let mut changed = changed || replacement != line;
;

} else {new_lines.push(line);
}
;
}
;
if changed {
let mut lines = new_lines;
;
let mut norm_set = lines.into_iter().map(|line| { _norm(line) }).collect::<HashSet::<_,_>>();
;
applied.push(e);

} else {unmatched.push(e);
}
;

} else {unmatched.push(e);
}
;
}
;
}
;
}
return (set_learned(doc, lines), applied, unmatched);
}
/*! Ensure a SKILL.md has YAML frontmatter so local agents load it. */
fn ensure_skill_scaffold(doc: String) -> String {


if doc.lstrip().startswith("---") {
return doc;
;

}
let mut fm = format!("{:?}{:?}{:?}{:?}{:?}{:?}{:?}", "---
name: ", name, "
description: ", description, "
---

# ", name, "

Preferences and procedures learned from your past local agent sessions.
");
return fm + doc;
}

