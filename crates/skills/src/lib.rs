//! skills — Markdown + JSON front-matter skill pack loader for Onyx.
//!
//! Specification & design:
//! - Discovers skill packs in `~/.onyx/skills/<name>/SKILL.md`.
//! - Parses JSON front-matter between `---` fences (no external YAML dependency):
//!   `{"name": "...", "description": "...", "tools_allowed": ["..."], "model_hint": "..."}`.
//! - Body instructions are injected into the system prompt when selected via `/skill <name>`
//!   or keyword triggers.
//! - Scope enforcement: skills declaring `tools_allowed` restrict the active tool set
//!   via `ToolCollection::retain()`. A skill cannot widen permissions beyond what the parent
//!   collection permits.

use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tools::ToolCollection;


#[derive(Debug, Clone, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub tools_allowed: Vec<String>,
    pub model_hint: Option<String>,
    pub instructions: String,
}

impl Skill {
    /// Parse a SKILL.md file with JSON front-matter between `---` delimiters.
    pub fn parse(content: &str) -> Result<Self, String> {
        let trimmed = content.trim_start();
        if !trimmed.starts_with("---") {
            return Err("SKILL.md must start with `---` front-matter delimiter".into());
        }
        let after_first = &trimmed[3..];
        let end_idx = after_first
            .find("\n---")
            .ok_or_else(|| "missing closing `---` front-matter delimiter".to_string())?;

        let header_str = &after_first[..end_idx].trim();
        let body = after_first[end_idx + 4..].trim_start_matches('\n').trim();

        let meta: Value = serde_json::from_str(header_str)
            .map_err(|e| format!("parse JSON front-matter: {e}"))?;

        let name = meta
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing `name` in front-matter".to_string())?
            .to_string();

        let description = meta
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let tools_allowed = meta
            .get("tools_allowed")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let model_hint = meta
            .get("model_hint")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(Skill {
            name,
            description,
            tools_allowed,
            model_hint,
            instructions: body.to_string(),
        })
    }

    /// Apply this skill to a tool collection (restricts to tools_allowed if non-empty).
    pub fn apply_to_tools(&self, tools: &mut ToolCollection) {
        if !self.tools_allowed.is_empty() {
            let refs: Vec<&str> = self.tools_allowed.iter().map(|s| s.as_str()).collect();
            tools.retain(&refs);
        }
    }

    /// Render updated system prompt with skill instructions appended.
    pub fn augment_system_prompt(&self, base_system: &str) -> String {
        format!(
            "{}\n\n[Active Skill: {}]\n{}\n{}",
            base_system,
            self.name,
            self.description,
            self.instructions
        )
    }
}

/// Catalog of discovered skills.
#[derive(Debug, Default, Clone)]
pub struct SkillCatalog {
    pub skills: HashMap<String, Skill>,
}

impl SkillCatalog {
    pub fn new() -> Self {
        Self {
            skills: HashMap::new(),
        }
    }

    /// Scan a directory (e.g. `~/.onyx/skills`) for skill packs.
    pub fn scan_dir(&mut self, root: &Path) {
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                let skill_file = entry.path().join("SKILL.md");
                if skill_file.is_file() {
                    if let Ok(content) = fs::read_to_string(&skill_file) {
                        if let Ok(skill) = Skill::parse(&content) {
                            self.skills.insert(skill.name.clone(), skill);
                        }
                    }
                }
            }
        }
    }

    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_skill_markdown() {
        let doc = r#"---
{
    "name": "git-helper",
    "description": "Assists with git commits",
    "tools_allowed": ["bash", "terminate"]
}
---
# Git Helper Instructions
Always craft clean commit messages following Conventional Commits.
"#;
        let skill = Skill::parse(doc).unwrap();
        assert_eq!(skill.name, "git-helper");
        assert_eq!(skill.description, "Assists with git commits");
        assert_eq!(skill.tools_allowed, vec!["bash", "terminate"]);
        assert!(skill.instructions.contains("Conventional Commits"));
    }

    #[test]
    fn skill_restricts_tool_collection() {
        struct DummyTool(&'static str);
        impl tools::Tool for DummyTool {
            fn name(&self) -> &str { self.0 }
            fn description(&self) -> &str { "" }
            fn execute(&self, _a: &tools::Args) -> Result<tools::Output, tools::ToolError> {
                Ok(tools::Output::ok("ok"))
            }
        }

        let mut col = ToolCollection::new();
        col.add(DummyTool("bash"));
        col.add(DummyTool("python"));
        col.add(DummyTool("terminate"));

        let skill = Skill {
            name: "safe-bash".into(),
            description: "only bash allowed".into(),
            tools_allowed: vec!["bash".into(), "terminate".into()],
            model_hint: None,
            instructions: "body".into(),
        };

        skill.apply_to_tools(&mut col);
        assert_eq!(col.names(), vec!["bash".to_string(), "terminate".to_string()]);
        assert!(col.get("python").is_none());
    }

    #[test]
    fn system_prompt_augmentation() {
        let skill = Skill {
            name: "reviewer".into(),
            description: "Code reviewer".into(),
            tools_allowed: vec![],
            model_hint: None,
            instructions: "Be thorough and check safety.".into(),
        };
        let augmented = skill.augment_system_prompt("Base system.");
        assert!(augmented.contains("Base system."));
        assert!(augmented.contains("[Active Skill: reviewer]"));
        assert!(augmented.contains("Be thorough and check safety."));
    }
}
