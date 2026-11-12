use serde::{Deserialize, Serialize};

/// The primary event types that yield coverage or investigation evidence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    /// Direct inspection of file contents (e.g., via read_file tool).
    FileRead,
    /// Modification of file contents (e.g., via editor or write tools).
    FileModify,
    /// Appearance in search results (e.g., grep, file search). Never implies inspection.
    SearchHit,
    /// Build or compile verification (e.g., cargo check, build scripts).
    BuildVerification,
    /// Automated test execution (e.g., cargo test, jest, pytest).
    TestExecution,
    /// Process runtime or execution verification (e.g., running the binary, server probe).
    RuntimeVerification,
    /// User or manual verification confirmation.
    ManualVerification,
}

/// Verification status or outcome.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Passed,
    Failed,
    Partial,
    Pending,
}

/// Verification scope definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationScope {
    /// Applies strictly to the file itself.
    File,
    /// Applies to an entire module or directory.
    Module(String),
    /// Applies to a crate or package.
    Package(String),
    /// Applies to targeted integration tests or specific test cases.
    TargetedTest(String),
    /// Workspace-wide or end-to-end test suite.
    WorkspaceSuite,
}

/// An audit event capturing a concrete piece of evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageEvent {
    pub id: String,
    pub timestamp: u64,
    pub session_id: Option<String>,
    pub source: EvidenceSource,
    pub relative_path: Option<String>,
    pub content_hash: Option<String>,
    pub command: Option<String>,
    pub status: VerificationStatus,
    pub scope: VerificationScope,
    pub details: Option<String>,
}

impl CoverageEvent {
    pub fn new(
        source: EvidenceSource,
        status: VerificationStatus,
        scope: VerificationScope,
    ) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let id = format!("{}-{}", now, fastrand_or_hash(now));

        Self {
            id,
            timestamp: now,
            session_id: None,
            source,
            relative_path: None,
            content_hash: None,
            command: None,
            status,
            scope,
            details: None,
        }
    }

    pub fn with_file(mut self, path: impl Into<String>, hash: Option<String>) -> Self {
        self.relative_path = Some(path.into());
        self.content_hash = hash;
        self
    }

    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_command(mut self, cmd: impl Into<String>) -> Self {
        self.command = Some(cmd.into());
        self
    }

    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }
}

fn fastrand_or_hash(val: u64) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(val.to_le_bytes());
    let res = hex::encode(hasher.finalize());
    res[..8].to_string()
}
