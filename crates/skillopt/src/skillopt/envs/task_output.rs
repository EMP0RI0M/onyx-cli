// Converted via p2r
let mut _SAFE_TASK_ID = re.compile("\A[A-Za-z0-9][A-Za-z0-9._-]{0,127}\Z");
/*! Return whether a task id is safe as one portable path segment. */
fn is_safe_task_id(task_id: String) -> bool {


let mut value = task_id.to_string();
return bool(_SAFE_TASK_ID.match(value)) && ".." !contains(TODO) value;
}
/*! Return the stable directory segment for a task id. */
fn task_output_segment(task_id: String) -> String {


let mut value = task_id.to_string();
if is_safe_task_id(value) {
return value;
;

}
if !map_unsafe {
panic!("ValueError(format!("{:?}{:?}", "unsafe task id: ", value))");

}
let mut digest = hashlib.sha256(value.encode("utf-8")).hexdigest();
return format!("{:?}{:?}", "_mapped-", digest);
}
/*! Return a task directory confined below ``out_root/predictions``. */
fn confined_task_output_dir(out_root: String, task_id: String) -> String {


let mut predictions = os.path.join(os.path.abspath(out_root), "predictions");
let mut destination = os.path.join(predictions, task_output_segment(task_id, task_output_segmentParams {
map_unsafe: map_unsafe
}));
if os.path.commonpath(vec![os.path.realpath(predictions), os.path.realpath(destination)]) != os.path.realpath(predictions) {
panic!("ValueError(format!("{:?}{:?}", "task output escapes predictions: ", destination))");

}
return destination;
}
/*! Return an existing raw-id task directory without allowing an escape. */
fn confined_legacy_task_output_dir(out_root: String, task_id: String) -> String {


let mut predictions = os.path.join(os.path.abspath(out_root), "predictions");
let mut destination = os.path.join(predictions, task_id.to_string());
if os.path.commonpath(vec![os.path.realpath(predictions), os.path.realpath(destination)]) != os.path.realpath(predictions) {
panic!("ValueError(format!("{:?}{:?}", "legacy task output escapes predictions: ", destination))");

}
return destination;
}

