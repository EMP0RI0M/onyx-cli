//! Capability metadata and registry indexing.
//!
//! Exposes lightweight capability representation without duplicating tool
//! execution logic or schemas.

use serde::{Deserialize, Serialize};

/// The kind of capability available to the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    /// A native tool registered in ToolCollection.
    Tool,
    /// An external Model Context Protocol tool.
    Mcp,
    /// An installed skill / instruction playbook.
    Skill,
    /// A system command or utility.
    Command,
}

impl std::fmt::Display for CapabilityKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapabilityKind::Tool => write!(f, "tool"),
            CapabilityKind::Mcp => write!(f, "mcp"),
            CapabilityKind::Skill => write!(f, "skill"),
            CapabilityKind::Command => write!(f, "command"),
        }
    }
}

/// Lightweight capability descriptor for retrieval and progressive disclosure (L0).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    /// Unique identifier (e.g., tool name or skill name).
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Short summary of what this capability does.
    pub description: String,
    /// The capability category.
    pub kind: CapabilityKind,
    /// Searchable tags and categorization keywords.
    pub tags: Vec<String>,
}

impl Capability {
    pub fn new(id: impl Into<String>, name: impl Into<String>, description: impl Into<String>, kind: CapabilityKind) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            kind,
            tags: Vec::new(),
        }
    }

    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }
}
