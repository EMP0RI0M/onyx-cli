//! skills — Markdown + YAML/JSON front-matter skill pack loader for Onyx.
//!
//! Specification & design:
//! - Discovers skill packs in `~/.onyx/skills/<name>/SKILL.md`.
//! - Parses front-matter between `---` fences (supports both JSON `{...}` and key-value YAML `key: value`).
//! - Body instructions are injected into the system prompt when selected via `/skill <name>`
//!   or keyword triggers.
//! - Scope enforcement: skills declaring `tools_allowed` restrict the active tool set
//!   via `ToolCollection::retain()`. A skill cannot widen permissions beyond what the parent
//!   collection permits.
//! - Direct integration with `skillopt` for content hashing, patch application, and autonomous optimization.

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
    /// Parse a SKILL.md file with YAML or JSON front-matter between `---` delimiters.
    pub fn parse(content: &str) -> Result<Self, String> {
        let trimmed = content.trim_start();
        if !trimmed.starts_with("---") {
            return Err("SKILL.md must start with `---` front-matter delimiter".into());
        }
        let after_first = &trimmed[3..];
        let end_idx = after_first
            .find("\n---")
            .ok_or_else(|| "missing closing `---` front-matter delimiter".to_string())?;

        let header_str = after_first[..end_idx].trim();
        let body = after_first[end_idx + 4..].trim_start_matches('\n').trim();

        let (name, description, tools_allowed, model_hint) = if header_str.starts_with('{') {
            // JSON front-matter
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

            (name, description, tools_allowed, model_hint)
        } else {
            // Line-based YAML key-value parser (no heavy yaml-rust dependency)
            let mut name = None;
            let mut description = String::new();
            let mut tools_allowed = Vec::new();
            let mut model_hint = None;

            for line in header_str.lines() {
                let trimmed_line = line.trim();
                if trimmed_line.is_empty() || trimmed_line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = trimmed_line.split_once(':') {
                    let k = k.trim();
                    let v = v.trim().trim_matches('"').trim_matches('\'');
                    match k {
                        "name" => name = Some(v.to_string()),
                        "description" => description = v.to_string(),
                        "model_hint" => model_hint = Some(v.to_string()),
                        "tools_allowed" => {
                            if v.starts_with('[') && v.ends_with(']') {
                                let inner = &v[1..v.len() - 1];
                                for item in inner.split(',') {
                                    let s = item.trim().trim_matches('"').trim_matches('\'');
                                    if !s.is_empty() {
                                        tools_allowed.push(s.to_string());
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }

            let name = name.ok_or_else(|| "missing `name` in YAML front-matter".to_string())?;
            (name, description, tools_allowed, model_hint)
        };

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

    /// Compute deterministic content hash using SkillOpt hashing engine.
    pub fn content_hash(&self) -> String {
        skillopt::skillopt::utils::scoring::skill_hash(&self.instructions)
    }

    /// Apply an optimization patch from SkillOpt directly to this skill's instructions.
    pub fn apply_optimization_patch(&mut self, patch: &skillopt::skillopt::types::Patch) {
        self.instructions = skillopt::skillopt::engine::ReflActPipeline::apply_patch(
            &self.instructions,
            patch,
        );
    }
}

/// Catalog of discovered skills.
#[derive(Debug, Default, Clone)]
pub struct SkillCatalog {
    pub skills: HashMap<String, Skill>,
    /// Directory the catalog was scanned from, used to write updates back.
    root: Option<std::path::PathBuf>,
}

impl SkillCatalog {
    pub fn new() -> Self {
        Self {
            skills: HashMap::new(),
            root: None,
        }
    }

    /// Read every skill pack under `root` in one step.
    pub fn load_root(root: &Path) -> Self {
        let mut catalog = Self::new();
        catalog.scan_dir(root);
        catalog
    }

    /// Discover all default skill search directories honoring ONYX_SKILLS_PATH, ONYX_HOME,
    /// and standard installed locations (~/.onyx/skills, ~/.gemini/skills, ~/.agents/skills, ~/.gemini/antigravity-cli/builtin/skills).
    pub fn default_skills_dirs() -> Vec<std::path::PathBuf> {
        let mut dirs = Vec::new();

        // 1. Explicit colon-separated ONYX_SKILLS_PATH (overrides standard search if set)
        if let Ok(paths) = std::env::var("ONYX_SKILLS_PATH") {
            for p in paths.split(':') {
                let p = p.trim();
                if !p.is_empty() {
                    dirs.push(std::path::PathBuf::from(p));
                }
            }
            if !dirs.is_empty() {
                return dirs.into_iter().filter(|d| d.is_dir()).collect();
            }
        }

        // 2. ONYX_HOME/skills or default ~/.onyx/skills
        if let Ok(h) = std::env::var("ONYX_HOME") {
            if !h.is_empty() {
                dirs.push(std::path::PathBuf::from(h).join("skills"));
            }
        } else if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
            dirs.push(home.join(".onyx/skills"));
        }

        // 3. System and standard agent skills directories
        if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
            dirs.push(home.join(".gemini/skills"));
            dirs.push(home.join(".agents/skills"));
            dirs.push(home.join(".gemini/antigravity-cli/builtin/skills"));
        }

        // Filter to existing directories and dedup
        let mut unique = Vec::new();
        for d in dirs {
            if d.is_dir() && !unique.contains(&d) {
                unique.push(d);
            }
        }
        unique
    }

    /// Scan all default search directories in order, merging all skills into this catalog.
    pub fn scan_all_default(&mut self) {
        for dir in Self::default_skills_dirs() {
            self.scan_dir(&dir);
        }
    }

    /// Scan a directory (e.g. `~/.onyx/skills`) for skill packs.
    pub fn scan_dir(&mut self, root: &Path) {        self.root = Some(root.to_path_buf());
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

    /// Every loaded skill, in insertion order. Used by retrieval to index the whole
    /// catalog (name + description + instructions) so a terse intent prompt can
    /// match a skill the operator never named explicitly.
    pub fn all(&self) -> Vec<&Skill> {
        self.skills.values().collect()
    }

    pub fn count(&self) -> usize {
        self.skills.len()
    }

    /// Directory this catalog was scanned from, when known.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Locate a skill's `SKILL.md`, searching every subdirectory so a skill whose
    /// folder name differs from its front-matter `name` still resolves.
    ///
    /// Searches the directory this catalog was scanned from *first* (so an explicit
    /// single-root catalog stays fast and correct), then falls back to every default
    /// search path — which is what makes writes land on the right file after a
    /// multi-root `scan_all_default()`, where `root` only remembers the last dir.
    pub fn skill_path(&self, name: &str) -> Option<std::path::PathBuf> {
        let mut roots: Vec<std::path::PathBuf> = Vec::new();
        if let Some(r) = self.root.as_ref() {
            roots.push(r.clone());
        }
        for d in Self::default_skills_dirs() {
            if !roots.contains(&d) {
                roots.push(d);
            }
        }

        for root in roots {
            let entries = match fs::read_dir(&root) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let candidate = entry.path().join("SKILL.md");
                if !candidate.is_file() {
                    continue;
                }
                let Ok(content) = fs::read_to_string(&candidate) else { continue };
                if let Ok(skill) = Skill::parse(&content) {
                    if skill.name == name {
                        return Some(candidate);
                    }
                }
            }
        }
        None
    }

    /// Write a skill's instructions back to its `SKILL.md`, preserving the
    /// front-matter. Automatically archives the previous version to `.onyx/skills/backups/`.
    /// Returns the previous content hash so callers can report exactly what changed.
    ///
    /// Written atomically — temp file beside the target, then rename — so an
    /// interrupted save cannot leave a half-written skill.
    pub fn persist_instructions(&self, name: &str, instructions: &str) -> Result<String, String> {
        let path = self
            .skill_path(name)
            .ok_or_else(|| format!("no SKILL.md found for `{name}`"))?;
        let original =
            fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let skill = Skill::parse(&original).map_err(|e| format!("parse {}: {e}", path.display()))?;
        let previous_hash = skill.content_hash();

        // 1. Archive snapshot to .onyx/skills/backups/
        if let Err(e) = Self::archive_backup(name, &previous_hash, &original) {
            eprintln!("warning: failed to write skill backup for {name}: {e}");
        }

        let front = front_matter_of(&original)?;
        let mut doc = String::with_capacity(front.len() + instructions.len() + 16);
        doc.push_str("---\n");
        doc.push_str(front.trim());
        doc.push_str("\n---\n");
        doc.push_str(instructions.trim_end());
        doc.push('\n');

        let tmp = path.with_file_name(".SKILL.md.onyx-tmp");
        fs::write(&tmp, &doc).map_err(|e| format!("write {}: {e}", tmp.display()))?;
        fs::rename(&tmp, &path).map_err(|e| format!("rename {}: {e}", path.display()))?;
        Ok(previous_hash)
    }

    /// Archive a previous version snapshot of a skill under `.onyx/skills/backups/`.
    pub fn archive_backup(name: &str, hash: &str, content: &str) -> Result<(), String> {
        let root = std::env::var("ONYX_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::var("HOME")
                    .map(|h| std::path::Path::new(&h).join(".onyx"))
                    .unwrap_or_else(|_| std::path::PathBuf::from(".onyx"))
            });

        let backup_dir = root.join("skills").join("backups");
        fs::create_dir_all(&backup_dir).map_err(|e| format!("create backup dir: {e}"))?;

        let filename = format!("{name}.{hash}.md");
        let target = backup_dir.join(&filename);
        let tmp = backup_dir.join(format!(".{filename}.tmp"));

        fs::write(&tmp, content).map_err(|e| format!("write backup: {e}"))?;
        fs::rename(&tmp, &target).map_err(|e| format!("rename backup: {e}"))?;
        Ok(())
    }

    /// Roll back a skill to a specific previous hash or the most recent backup.
    pub fn rollback_skill(&self, name: &str, target_hash: Option<&str>) -> Result<String, String> {
        let path = self
            .skill_path(name)
            .ok_or_else(|| format!("no SKILL.md found for `{name}`"))?;

        let root = std::env::var("ONYX_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::var("HOME")
                    .map(|h| std::path::Path::new(&h).join(".onyx"))
                    .unwrap_or_else(|_| std::path::PathBuf::from(".onyx"))
            });

        let backup_dir = root.join("skills").join("backups");
        if !backup_dir.exists() {
            return Err("no backups directory found".to_string());
        }

        let backup_file = match target_hash {
            Some(h) => {
                let candidate = backup_dir.join(format!("{name}.{h}.md"));
                if !candidate.exists() {
                    return Err(format!("backup for hash `{h}` not found"));
                }
                candidate
            }
            None => {
                // Find most recent backup for this skill name
                let mut matches = Vec::new();
                for entry in fs::read_dir(&backup_dir).map_err(|e| e.to_string())?.flatten() {
                    let fname = entry.file_name().to_string_lossy().to_string();
                    if fname.starts_with(&format!("{name}.")) && fname.ends_with(".md") {
                        if let Ok(meta) = entry.metadata() {
                            let mod_time = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                            matches.push((entry.path(), mod_time));
                        }
                    }
                }
                matches.sort_by(|a, b| b.1.cmp(&a.1));
                matches
                    .into_iter()
                    .next()
                    .map(|(p, _)| p)
                    .ok_or_else(|| format!("no backup found for skill `{name}`"))?
            }
        };

        let backup_content = fs::read_to_string(&backup_file).map_err(|e| format!("read backup: {e}"))?;
        let restored_skill = Skill::parse(&backup_content).map_err(|e| format!("parse backup: {e}"))?;
        let restored_hash = restored_skill.content_hash();

        // Atomically write restored content back
        let tmp = path.with_file_name(".SKILL.md.onyx-tmp");
        fs::write(&tmp, &backup_content).map_err(|e| format!("write rollback: {e}"))?;
        fs::rename(&tmp, &path).map_err(|e| format!("rename rollback: {e}"))?;

        Ok(restored_hash)
    }
}

/// Extract the raw front-matter block between the leading `---` fences.
fn front_matter_of(content: &str) -> Result<String, String> {
    let trimmed = content.trim_start();
    let after = trimmed
        .strip_prefix("---")
        .ok_or_else(|| "SKILL.md must start with `---`".to_string())?;
    let end = after.find("\n---").ok_or_else(|| "missing closing `---`".to_string())?;
    Ok(after[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_skill_markdown_json() {
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
        let mut skill = Skill::parse(doc).unwrap();
        assert_eq!(skill.name, "git-helper");
        assert_eq!(skill.description, "Assists with git commits");
        assert_eq!(skill.tools_allowed, vec!["bash", "terminate"]);
        assert!(skill.instructions.contains("Conventional Commits"));

        // Verify fused SkillOpt operations
        assert_eq!(skill.content_hash().len(), 16);

        let patch = skillopt::skillopt::types::Patch {
            edits: vec![skillopt::skillopt::types::Edit {
                op: skillopt::skillopt::types::EditOp::Append,
                content: "Keep subject line under 50 characters.".to_string(),
                ..Default::default()
            }],
            reasoning: "Rule enforcement".to_string(),
            ranking_details: None,
        };
        skill.apply_optimization_patch(&patch);
        assert!(skill.instructions.contains("Keep subject line under 50 characters."));
    }

    #[test]
    fn parse_valid_skill_markdown_yaml() {
        let doc = r#"---
name: wrangler
description: "Cloudflare Workers CLI"
tools_allowed: ["bash", "read_file"]
---
# Wrangler CLI
Always prefer retrieval over pre-training.
"#;
        let skill = Skill::parse(doc).unwrap();
        assert_eq!(skill.name, "wrangler");
        assert_eq!(skill.description, "Cloudflare Workers CLI");
        assert_eq!(skill.tools_allowed, vec!["bash", "read_file"]);
        assert!(skill.instructions.contains("Always prefer retrieval"));
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

    #[test]
    fn persist_round_trip_preserves_front_matter() {
        let root = std::env::temp_dir().join(format!("onyx-skills-{}", std::process::id()));
        let dir = root.join("git-helper");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            "---\n{\"name\": \"git-helper\", \"description\": \"Commits\", \"tools_allowed\": [\"bash\"]}\n---\nWrite clean commit messages.\n",
        )
        .unwrap();

        let mut catalog = SkillCatalog::new();
        catalog.scan_dir(&root);
        assert_eq!(catalog.count(), 1);

        let path = catalog.skill_path("git-helper").expect("skill path resolves");
        let before = catalog.get("git-helper").unwrap().content_hash();
        let prev = catalog
            .persist_instructions("git-helper", "Write clean commit messages.\nKeep the subject under 50 characters.")
            .unwrap();
        assert_eq!(prev, before);

        // Reload from disk: front-matter intact, body updated.
        let mut reloaded = SkillCatalog::new();
        reloaded.scan_dir(&root);
        let skill = reloaded.get("git-helper").unwrap();
        assert_eq!(skill.description, "Commits");
        assert_eq!(skill.tools_allowed, vec!["bash"]);
        assert!(skill.instructions.contains("under 50 characters"));
        assert_ne!(skill.content_hash(), before);
        assert!(path.is_file());

        // Unknown names fail rather than silently writing elsewhere.
        assert!(catalog.persist_instructions("nope", "x").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn skill_path_resolves_across_multiple_roots() {
        // Regression: after scan_all_default(), `root` only remembers the LAST dir,
        // so skill_path must fall back to every default search path — otherwise a
        // skill owned by an earlier root could never be persisted.
        let base = std::env::temp_dir().join(format!("onyx-skills-multi-{}", std::process::id()));
        let first = base.join("a");
        let second = base.join("b");
        fs::create_dir_all(first.join("alpha")).unwrap();
        fs::create_dir_all(second.join("beta")).unwrap();
        fs::write(
            first.join("alpha/SKILL.md"),
            "---\n{\"name\": \"alpha\", \"description\": \"A\", \"tools_allowed\": [\"bash\"]}\n---\nBody A.\n",
        )
        .unwrap();
        fs::write(
            second.join("beta/SKILL.md"),
            "---\n{\"name\": \"beta\", \"description\": \"B\", \"tools_allowed\": [\"bash\"]}\n---\nBody B.\n",
        )
        .unwrap();

        // Point two default-search roots at our scratch dirs; `first` owns `alpha`.
        let mut catalog = SkillCatalog::new();
        catalog.scan_dir(&first);
        catalog.scan_dir(&second);
        // Force skill_path's fallback list to include `first` even though `root`
        // now equals `second`, by re-adding both via the env override as well.
        std::env::set_var("ONYX_SKILLS_PATH", format!("{}:{}", first.display(), second.display()));

        assert_eq!(catalog.root(), Some(second.as_path()), "root is the LAST scanned dir");

        // `alpha` lives in the FIRST dir, yet must still resolve and persist.
        let p = catalog.skill_path("alpha").expect("alpha resolves despite non-final root");
        assert_eq!(p.parent().unwrap().file_name().unwrap(), "alpha");
        catalog
            .persist_instructions("alpha", "Body A improved.")
            .expect("persist across multi-root");
        let on_disk = fs::read_to_string(first.join("alpha/SKILL.md")).unwrap();
        assert!(on_disk.contains("Body A improved."), "write landed in owning root");

        std::env::remove_var("ONYX_SKILLS_PATH");
        let _ = fs::remove_dir_all(&base);
    }
}
