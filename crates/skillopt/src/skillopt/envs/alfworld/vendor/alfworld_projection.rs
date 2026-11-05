// Converted via p2r
/*! Process raw model outputs into valid ALFWorld actions.

    Extracts text from ``<action>...</action>`` tags and validates that
    the response also contains ``<think>...</think>`` tags.

    Parameters
    ----------
    actions : list[str]
        Raw model outputs, one per environment.
    action_pools : list[list[str]]
        Admissible action lists per environment (unused but kept for API compat).

    Returns
    -------
    actions : list[str]
        Cleaned action strings.
    valids : list[int]
        1 if the action was successfully parsed, 0 otherwise.
     */
fn alfworld_projection(actions: Vec<String>, action_pools: Vec<Vec<String>>) -> () {


let mut valids = vec![0] * actions.len();
for i in (0..actions.len()) {
let mut original_str = actions[i];
;
let mut actions[mut i] = actions[i].lower();
;
let mut start_tag = "<action>";
;
let mut end_tag = "</action>";
;
let mut start_idx = actions[i].find(start_tag);
;
let mut end_idx = actions[i].find(end_tag);
;
try_it(|| {if start_idx == -1 || end_idx == -1 {
let mut actions[mut i] = actions[i][-30..];
;
continue;

}
;
let mut extracted_action = actions[i][start_idx + start_tag.len()..end_idx].strip().lower();
;
let mut actions[mut i] = extracted_action;
;
let mut valids[mut i] = 1;
});
 catch_it(|error_name| {
let mut actions[mut i] = actions[i][-30..];

});;
let mut think_start_idx = original_str.find("<think>");
;
let mut think_end_idx = original_str.find("</think>");
;
if think_start_idx == -1 || think_end_idx == -1 {
let mut valids[mut i] = 0;
;

}
;
if re.search("[\u4e00-\u9fff]", original_str) {
let mut valids[mut i] = 0;
;

}
;
}
return (actions, valids);
}

