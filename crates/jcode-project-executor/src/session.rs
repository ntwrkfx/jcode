use serde::{Deserialize, Serialize};

pub const SESSION_SCHEMA_VERSION: &str = "project-executor-session/v1";
pub const WORKTREE_BINDING_SCHEMA_VERSION: &str = "worktree-binding/v1";
pub const WORKTREE_INSPECTION_SCHEMA_VERSION: &str = "worktree-inspection/v1";
pub const PROVIDER_NAME: &str = "project-executor";
pub const IMPLEMENTATION_NAME: &str = "jcode-derived-executor/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SessionState {
    Ready,
    Closed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WorktreeMode {
    Existing,
    Managed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WorktreeOwnership {
    External,
    Harness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessMode {
    Read,
    Write,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeBinding {
    pub schema_version: String,
    pub mode: WorktreeMode,
    pub ownership: WorktreeOwnership,
    pub path: String,
    pub repository: String,
    pub resolved_sha: String,
    pub access_mode: AccessMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeInspection {
    pub schema_version: String,
    pub path: String,
    pub repository: String,
    pub branch: String,
    pub head_sha: String,
    pub origin: Option<String>,
    pub dirty: bool,
    pub owner: String,
    pub writer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCreateRequest {
    pub execution_id: String,
    pub work_identity: String,
    pub repository: String,
    pub base_sha: String,
    pub worktree_path: Option<String>,
    pub access_mode: Option<AccessMode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRecord {
    pub schema_version: String,
    pub execution_id: String,
    pub provider: String,
    pub implementation: String,
    pub implementation_revision: String,
    pub work_identity: String,
    pub repository: String,
    pub repository_path: String,
    pub base_sha: String,
    pub branch: String,
    pub workspace: String,
    #[serde(default)]
    pub worktree_binding: Option<WorktreeBinding>,
    pub state: SessionState,
    pub created_at: u64,
    pub closed_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInspection {
    pub schema_version: String,
    pub execution_id: String,
    pub provider: String,
    pub implementation: String,
    pub implementation_revision: String,
    pub work_identity: String,
    pub repository: String,
    pub repository_path: String,
    pub base_sha: String,
    pub branch: String,
    pub workspace: String,
    pub worktree_binding: WorktreeBinding,
    pub state: SessionState,
    pub created_at: u64,
    pub closed_at: Option<u64>,
    pub head_sha: String,
    pub clean: bool,
}
