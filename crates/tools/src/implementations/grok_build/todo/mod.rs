//! Todo tool — maintains structured checklist tasks for the agent session.
use crate::{Args, Output, Tool, ToolError};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct TodoItem {
    pub id: usize,
    pub task: String,
    pub status: String, // "pending" | "in_progress" | "completed"
}

#[derive(Default, Clone)]
pub struct TodoTool {
    todos: Arc<Mutex<Vec<TodoItem>>>,
}

impl TodoTool {
    pub fn new() -> Self {
        Self {
            todos: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Tool for TodoTool {
    fn name(&self) -> &str {
        "todo"
    }

    fn description(&self) -> &str {
        "Manage session todo checklist. Args: action ('list'|'add'|'update'|'clear'), task (string for add), id (int for update), status ('pending'|'in_progress'|'completed')."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec!["action".into(), "task".into(), "id".into(), "status".into()]
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("list");
        let mut list = self.todos.lock().unwrap();

        match action {
            "add" => {
                let task = args.get("task").and_then(|v| v.as_str()).ok_or_else(|| {
                    ToolError::BadArgs("missing 'task' for add".into())
                })?;
                let next_id = list.len() + 1;
                list.push(TodoItem {
                    id: next_id,
                    task: task.to_string(),
                    status: "pending".to_string(),
                });
                Ok(Output::ok(format!("added todo #{next_id}: {task}")))
            }
            "update" => {
                let id = args.get("id").and_then(|v| v.as_u64()).ok_or_else(|| {
                    ToolError::BadArgs("missing 'id' for update".into())
                })? as usize;
                let status = args.get("status").and_then(|v| v.as_str()).unwrap_or("completed");
                if let Some(item) = list.iter_mut().find(|t| t.id == id) {
                    item.status = status.to_string();
                    Ok(Output::ok(format!("updated todo #{id} status to '{status}'")))
                } else {
                    Err(ToolError::BadArgs(format!("todo id #{id} not found")))
                }
            }
            "clear" => {
                list.clear();
                Ok(Output::ok("todo list cleared"))
            }
            "list" | _ => {
                if list.is_empty() {
                    Ok(Output::ok("no active todos"))
                } else {
                    let mut s = String::new();
                    for t in list.iter() {
                        let mark = match t.status.as_str() {
                            "completed" => "[✓]",
                            "in_progress" => "[◐]",
                            _ => "[ ]",
                        };
                        s.push_str(&format!("{mark} #{}: {} ({})\n", t.id, t.task, t.status));
                    }
                    Ok(Output::ok(s.trim_end().to_string()))
                }
            }
        }
    }
}
