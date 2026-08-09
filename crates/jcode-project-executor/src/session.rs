use serde::{Deserialize, Serialize};

pub const SESSION_SCHEMA_VERSION: &str = "project-executor-session/v1";
pub const PROVIDER_NAME: &str = "project-executor";
pub const IMPLEMENTATION_NAME: &str = "jcode-derived-executor/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SessionState {
    Ready,
    Closed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCreateRequest {
    pub execution_id: String,
    pub work_identity: String,
    pub repository: String,
    pub base_sha: String,
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
    pub state: SessionState,
    pub created_at: u64,
    pub closed_at: Option<u64>,
    pub head_sha: String,
    pub clean: bool,
}
