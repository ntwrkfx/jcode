use crate::session::{
    CustodyAssessment, CustodyState, ExpectedMaterial, LocalMaterialState, SessionLifecycle,
    SessionRecord, SessionRunnability, WorkspaceOrigin, WorktreeBinding,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub const RECOVERY_RECEIPT_SCHEMA_VERSION: &str = "SessionRecoveryReceipt/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RecoveryReasonCode {
    ReadyWorkspaceMissing,
    RevisionMismatch,
    MaterialChanged,
    MaterialStateUnknown,
    CustodyContention,
    CustodyIndeterminate,
    CustodyReacquireFailed,
    SessionSchemaUnsupported,
    WorkspaceBindingMismatch,
    RecoveryProbeFailed,
    MaterialChangedDuringHandoff,
    LegacyRecordRequiresReconciliation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RecoveryHealth {
    Healthy,
    Degraded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRecoveryReceipt {
    pub schema_version: String,
    pub session_id: String,
    pub expected_executor_revision: String,
    pub observed_executor_revision: String,
    pub expected_workspace: String,
    pub observed_workspace: Option<String>,
    pub expected_material: ExpectedMaterial,
    pub observed_material: MaterialObservation,
    pub custody: CustodyAssessment,
    pub result: SessionRunnability,
    pub reason_codes: Vec<RecoveryReasonCode>,
    pub observed_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoverySummary {
    pub health: RecoveryHealth,
    pub runnable: usize,
    pub quarantined: usize,
    pub failed_recovery: usize,
    pub receipts: Vec<SessionRecoveryReceipt>,
}

pub fn summarize_recovery(mut receipts: Vec<SessionRecoveryReceipt>) -> RecoverySummary {
    receipts.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    let runnable = receipts
        .iter()
        .filter(|r| r.result == SessionRunnability::Runnable)
        .count();
    let quarantined = receipts
        .iter()
        .filter(|r| r.result == SessionRunnability::Quarantined)
        .count();
    let failed_recovery = receipts
        .iter()
        .filter(|r| r.result == SessionRunnability::FailedRecovery)
        .count();
    let health = if quarantined == 0 && failed_recovery == 0 {
        RecoveryHealth::Healthy
    } else if runnable == 0 && quarantined == 0 && failed_recovery > 0 {
        RecoveryHealth::Failed
    } else {
        RecoveryHealth::Degraded
    };
    RecoverySummary {
        health,
        runnable,
        quarantined,
        failed_recovery,
        receipts,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedSessionRecord {
    pub record: SessionRecord,
    pub translated_from_legacy: bool,
    pub schema_understood: bool,
    pub initial_reason_codes: Vec<RecoveryReasonCode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryEvidence {
    pub lifecycle: SessionLifecycle,
    pub workspace_origin: WorkspaceOrigin,
    pub schema_understood: bool,
    pub revision_matches: bool,
    pub binding_matches: bool,
    pub material_matches: bool,
    pub material_sufficiently_known: bool,
    pub custody: CustodyAssessment,
    pub current_executor_instance_id: String,
    pub write_custody_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryDecision {
    pub result: SessionRunnability,
    pub reason_codes: Vec<RecoveryReasonCode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionRecordV1 {
    schema_version: String,
    execution_id: String,
    provider: String,
    implementation: String,
    implementation_revision: String,
    work_identity: String,
    repository: String,
    repository_path: String,
    base_sha: String,
    branch: String,
    workspace: String,
    #[serde(default)]
    worktree_binding: Option<WorktreeBinding>,
    state: SessionLifecycle,
    created_at: u64,
    closed_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct UnsupportedSessionEnvelope {
    execution_id: String,
    provider: String,
    implementation: String,
    implementation_revision: String,
    work_identity: String,
    repository: String,
    repository_path: String,
    base_sha: String,
    branch: String,
    workspace: String,
    #[serde(default)]
    worktree_binding: Option<WorktreeBinding>,
    #[serde(default)]
    state: Option<SessionLifecycle>,
    #[serde(default)]
    lifecycle: Option<SessionLifecycle>,
    created_at: u64,
    closed_at: Option<u64>,
}

pub fn decode_session_record(value: Value) -> Result<DecodedSessionRecord> {
    let schema = value
        .get("schema_version")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("session record missing schema_version"))?
        .to_owned();
    match schema.as_str() {
        "project-executor-session/v1" => {
            let legacy: SessionRecordV1 =
                serde_json::from_value(value).context("parse project-executor-session/v1")?;
            let (runnability, reasons) = match legacy.state {
                SessionLifecycle::Ready => (
                    SessionRunnability::Quarantined,
                    vec![RecoveryReasonCode::LegacyRecordRequiresReconciliation],
                ),
                SessionLifecycle::Closed => (SessionRunnability::Quarantined, vec![]),
                SessionLifecycle::Failed => (SessionRunnability::FailedRecovery, vec![]),
            };
            Ok(DecodedSessionRecord {
                record: SessionRecord {
                    schema_version: crate::session::SESSION_SCHEMA_VERSION.to_owned(),
                    execution_id: legacy.execution_id,
                    provider: legacy.provider,
                    implementation: legacy.implementation,
                    implementation_revision: legacy.implementation_revision,
                    work_identity: legacy.work_identity,
                    repository: legacy.repository,
                    repository_path: legacy.repository_path,
                    base_sha: legacy.base_sha.clone(),
                    branch: legacy.branch,
                    workspace: legacy.workspace,
                    worktree_binding: legacy.worktree_binding,
                    lifecycle: legacy.state,
                    runnability,
                    workspace_origin: WorkspaceOrigin::Legacy,
                    expected_material: ExpectedMaterial {
                        head_sha: legacy.base_sha,
                        local_material: LocalMaterialState::Unknown,
                    },
                    custody_assessment: Some(unknown_custody()),
                    created_at: legacy.created_at,
                    closed_at: legacy.closed_at,
                },
                translated_from_legacy: true,
                schema_understood: true,
                initial_reason_codes: reasons,
            })
        }
        "project-executor-session/v2" => {
            let mut record: SessionRecord =
                serde_json::from_value(value).context("parse project-executor-session/v2")?;
            if record.lifecycle == SessionLifecycle::Ready {
                record.runnability = SessionRunnability::Quarantined;
            }
            if let Some(custody) = record.custody_assessment.as_mut() {
                if custody.state == CustodyState::Confirmed {
                    *custody = unknown_custody();
                }
            }
            Ok(DecodedSessionRecord {
                record,
                translated_from_legacy: false,
                schema_understood: true,
                initial_reason_codes: vec![],
            })
        }
        other => {
            let envelope: UnsupportedSessionEnvelope = serde_json::from_value(value)
                .with_context(|| format!("parse unsupported session envelope: {other}"))?;
            let lifecycle = envelope.lifecycle.or(envelope.state).ok_or_else(|| {
                anyhow::anyhow!("unsupported session record missing lifecycle/state")
            })?;
            Ok(DecodedSessionRecord {
                record: SessionRecord {
                    schema_version: other.to_owned(),
                    execution_id: envelope.execution_id,
                    provider: envelope.provider,
                    implementation: envelope.implementation,
                    implementation_revision: envelope.implementation_revision,
                    work_identity: envelope.work_identity,
                    repository: envelope.repository,
                    repository_path: envelope.repository_path,
                    base_sha: envelope.base_sha.clone(),
                    branch: envelope.branch,
                    workspace: envelope.workspace,
                    worktree_binding: envelope.worktree_binding,
                    lifecycle,
                    runnability: SessionRunnability::Quarantined,
                    workspace_origin: WorkspaceOrigin::Legacy,
                    expected_material: ExpectedMaterial {
                        head_sha: envelope.base_sha,
                        local_material: LocalMaterialState::Unknown,
                    },
                    custody_assessment: Some(unknown_custody()),
                    created_at: envelope.created_at,
                    closed_at: envelope.closed_at,
                },
                translated_from_legacy: false,
                schema_understood: false,
                initial_reason_codes: vec![RecoveryReasonCode::SessionSchemaUnsupported],
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialObservation {
    pub head_sha: Option<String>,
    pub local_material: LocalMaterialState,
    pub observed_at: u64,
}

pub fn observe_git_material(workspace: &Path) -> MaterialObservation {
    let observed_at = now_millis();
    if !workspace.is_dir() {
        return MaterialObservation {
            head_sha: None,
            local_material: LocalMaterialState::Unknown,
            observed_at,
        };
    }
    let head = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(workspace)
        .args(["rev-parse", "HEAD"])
        .output();
    let Ok(head) = head else {
        return unknown_material(observed_at);
    };
    if !head.status.success() {
        return unknown_material(observed_at);
    }
    let head_sha = String::from_utf8_lossy(&head.stdout).trim().to_owned();
    if head_sha.len() != 40 || !head_sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return unknown_material(observed_at);
    }
    let status = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(workspace)
        .args(["status", "--porcelain=v1", "--untracked-files=normal"])
        .output();
    let Ok(status) = status else {
        return unknown_material(observed_at);
    };
    if !status.status.success() {
        return unknown_material(observed_at);
    }
    MaterialObservation {
        head_sha: Some(head_sha),
        local_material: if status.stdout.is_empty() {
            LocalMaterialState::None
        } else {
            LocalMaterialState::Present
        },
        observed_at,
    }
}

fn unknown_material(observed_at: u64) -> MaterialObservation {
    MaterialObservation {
        head_sha: None,
        local_material: LocalMaterialState::Unknown,
        observed_at,
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| u64::try_from(value.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

pub fn unknown_custody() -> CustodyAssessment {
    CustodyAssessment {
        state: CustodyState::Unknown,
        assessed_at: 0,
        executor_instance_id: None,
        generation: None,
        evidence_ref: None,
    }
}

pub fn derive_runnability(input: &RecoveryEvidence) -> RecoveryDecision {
    if !input.schema_understood {
        return quarantined(RecoveryReasonCode::SessionSchemaUnsupported);
    }
    if input.lifecycle != SessionLifecycle::Ready {
        return RecoveryDecision {
            result: match input.lifecycle {
                SessionLifecycle::Failed => SessionRunnability::FailedRecovery,
                _ => SessionRunnability::Quarantined,
            },
            reason_codes: vec![],
        };
    }
    if !input.revision_matches {
        return quarantined(RecoveryReasonCode::RevisionMismatch);
    }
    if !input.binding_matches {
        return quarantined(RecoveryReasonCode::WorkspaceBindingMismatch);
    }
    if !input.material_sufficiently_known {
        return quarantined(RecoveryReasonCode::MaterialStateUnknown);
    }
    if !input.material_matches {
        return quarantined(RecoveryReasonCode::MaterialChanged);
    }
    if input.write_custody_required {
        match input.custody.state {
            CustodyState::NotHeld => return quarantined(RecoveryReasonCode::CustodyContention),
            CustodyState::Unknown => {
                return quarantined(RecoveryReasonCode::CustodyIndeterminate);
            }
            CustodyState::Confirmed => {}
        }
        if input.custody.executor_instance_id.as_deref()
            != Some(input.current_executor_instance_id.as_str())
            || input.custody.generation.is_none()
        {
            return quarantined(RecoveryReasonCode::CustodyIndeterminate);
        }
    }
    RecoveryDecision {
        result: SessionRunnability::Runnable,
        reason_codes: vec![],
    }
}

fn quarantined(reason: RecoveryReasonCode) -> RecoveryDecision {
    RecoveryDecision {
        result: SessionRunnability::Quarantined,
        reason_codes: vec![reason],
    }
}
