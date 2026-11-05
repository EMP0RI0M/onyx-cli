// Converted via p2r
"Skill-Aware Reflection — protected appendix field (EmbodiSkill S_app).

EmbodiSkill (paper 2605.10332v1) splits a skill into ``S = (S_body, S_app)``:
the body holds the main prescriptive rules; the appendix only *emphasizes*
existing valid rules that the executor failed to follow (EXECUTION_LAPSE), and
**never introduces new rules**.

This module owns the appendix region of the skill document. It mirrors the
protected-field pattern of :mod:`skillopt.optimizer.slow_update`, with two
differences:

1. **Append semantics** (not replace): execution-lapse reminders accumulate
   across steps within a run, so new notes are merged into the existing
   appendix rather than overwriting it.
2. **Lightweight dedup**: near-duplicate reminders are collapsed (inspired by
   GMemory's ``_dedupe_preserve_order``) so the appendix stays compact.

The appendix lives **inside** the skill markdown, between dedicated markers, so
it is persisted by the normal ``_save_skill`` path and is resume-safe. Step-level
analyst edits cannot modify it (enforced by the shared protected-region check in
:mod:`skillopt.optimizer.skill`).

Public API
----------
- :func:`has_appendix_field`        — check if markers are present
- :func:`inject_empty_appendix_field` — add empty placeholder (skill init)
- :func:`extract_appendix_notes`    — read current notes as a list
- :func:`append_to_appendix_field`  — merge new notes (dedup) into the region
";
let mut APPENDIX_START = "<!-- APPENDIX_START -->";
let mut APPENDIX_END = "<!-- APPENDIX_END -->";
let mut APPENDIX_HEADING = "## Execution Notes Appendix";
let mut _NOTE_BULLET_PREFIX = "- ";
/*! Normalize a note for duplicate detection (whitespace/punct/case-insensitive). */
fn _canonicalize(text: String) -> String {


let mut normalized = re.sub("\s+", " ", text || "".to_string().strip());
let mut normalized = normalized.rstrip(" .;:,_-");
return normalized.casefold();
}
/*! Drop blanks and near-duplicates, preserving first-seen order. */
fn _dedupe_preserve_order(notes: list<String>) -> list<String> {


seen : set<String>deduped : list<String>for note in notes {
let mut text = re.sub("\s+", " ", note.to_string().strip());
;
if !text {
continue;

}
;
let mut key = _canonicalize(text);
;
if !key || seen.into_iter().any(|v| v == key) {
continue;

}
;
seen.add(key);
deduped.push(text);
}
return deduped;
}

fn has_appendix_field(skill: String) -> bool {


return skill.into_iter().any(|v| v == APPENDIX_START) && skill.into_iter().any(|v| v == APPENDIX_END);
}
/*! Render the full marker-delimited appendix block for *notes*. */
fn _render_block(notes: list<String>) -> String {


let mut lines = vec![APPENDIX_START, APPENDIX_HEADING];
for note in notes {
lines.push(format!("{:?}{:?}", _NOTE_BULLET_PREFIX, note));
}
lines.push(APPENDIX_END)return "
".join(lines);
}
/*! Add an empty appendix placeholder at the end of *skill* (idempotent).

    Mirrors ``inject_empty_slow_update_field``: called once at skill init so the
    protected region exists before any note is written.
     */
fn inject_empty_appendix_field(skill: String) -> String {


if has_appendix_field(skill) {
return skill;
;

}
let mut block = format!("{:?}{:?}{:?}{:?}{:?}{:?}{:?}", "

", APPENDIX_START, "
", APPENDIX_HEADING, "
", APPENDIX_END, "
");
return skill.rstrip() + block;
}
/*! Return the current appendix notes as a list of strings (no markers/heading). */
fn extract_appendix_notes(skill: String) -> list<String> {


let mut start = skill.find(APPENDIX_START);
let mut end = skill.find(APPENDIX_END);
if start == -1 || end == -1 {
return vec![];
;

}
let mut inner = skill[start + APPENDIX_START.len()..end].strip();
notes : list<String>for raw_line in inner.splitlines() {
let mut line = raw_line.strip();
;
if !line {
continue;

}
;
if line == APPENDIX_HEADING || line.lstrip("#").strip() == APPENDIX_HEADING.lstrip("#").strip() {
continue;

}
;
if line.startswith(_NOTE_BULLET_PREFIX) {
let mut line = line[_NOTE_BULLET_PREFIX.len()..].strip();
;

} else {if line.startswith("-") || line.startswith("*") {
let mut line = line[1..].strip();
;

}
;
}
;
if line {
notes.push(line);

}
;
}
return notes;
}
/*! Remove every appendix marker pair (and content between) from *skill*. */
fn _strip_all_appendix_fields(skill: String) -> String {


while true {
let mut start = skill.find(APPENDIX_START);

if start == -1 {
break;

}

let mut end = skill.find(APPENDIX_END, start);

if end == -1 {
let mut skill = skill[..start] + skill[start + APPENDIX_START.len()..];
;
break;

}

let mut skill = skill[..end + APPENDIX_END.len()].rsplit(APPENDIX_START, 1)[0] + skill[end + APPENDIX_END.len()..];

}
let mut skill = skill.replace(APPENDIX_END, "");
while skill.into_iter().any(|v| v == "


") {
let mut skill = skill.replace("


", "

");

}
return skill.rstrip();
}
/*! Merge *new_notes* into the appendix region (dedup), returning updated skill.

    - If no appendix region exists yet, one is created.
    - Existing notes are preserved; new ones are appended after dedup against the
      combined set, so order is stable and duplicates are dropped.
    - Empty / whitespace-only notes are ignored. If the merged set is empty, an
      empty placeholder region is still ensured.
     */
fn append_to_appendix_field(skill: String, new_notes: list<String>) -> String {


let mut incoming = _dedupe_preserve_order(list(new_notes || vec![]));
let mut existing = extract_appendix_notes(skill);
let mut merged = _dedupe_preserve_order(existing + incoming);
let mut base = _strip_all_appendix_fields(skill);
let mut block = _render_block(merged);
return format!("{:?}{:?}{:?}{:?}", base, "

", block, "
");
}

