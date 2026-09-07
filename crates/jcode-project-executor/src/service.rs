use crate::protocol::{
    EXECUTOR_PROTOCOL_VERSION, ExecutorCommand, ExecutorRequest, ExecutorResponse,
};
use crate::session::SessionCreateRequest;
use crate::supervisor::ProjectExecutorSupervisor;
use anyhow::{Result, bail};
use serde_json::{json, to_value};
use std::time::Duration;

const MAX_WAIT_SECONDS: f64 = 600.0;

pub async fn handle_executor_request(
    supervisor: &ProjectExecutorSupervisor,
    request: ExecutorRequest,
) -> ExecutorResponse {
    let id = request.id.clone();
    match handle_inner(supervisor, request).await {
        Ok(result) => ExecutorResponse::success(id, result),
        Err(error) => ExecutorResponse::failure(id, error.to_string()),
    }
}

async fn handle_inner(
    supervisor: &ProjectExecutorSupervisor,
    request: ExecutorRequest,
) -> Result<serde_json::Value> {
    if request.protocol != EXECUTOR_PROTOCOL_VERSION {
        bail!("unsupported executor protocol: {}", request.protocol);
    }
    if request.id.is_empty() {
        bail!("request id must not be empty");
    }
    match request.command {
        ExecutorCommand::WorktreeList => Ok(to_value(supervisor.list_worktrees().await?)?),
        ExecutorCommand::WorktreeInspect { path } => {
            Ok(to_value(supervisor.inspect_worktree(&path).await?)?)
        }
        ExecutorCommand::SessionCreate {
            execution_id,
            work_identity,
            repository,
            base_sha,
            worktree_path,
            access_mode,
            authorization_digest,
        } => Ok(to_value(
            supervisor
                .create_session(SessionCreateRequest {
                    execution_id,
                    work_identity,
                    repository,
                    base_sha,
                    worktree_path,
                    access_mode,
                    authorization_digest,
                })
                .await?,
        )?),
        ExecutorCommand::SessionInspect { execution_id } => {
            Ok(to_value(supervisor.inspect_session(&execution_id).await?)?)
        }
        ExecutorCommand::RepoPatchApply {
            execution_id,
            repository,
            base_sha,
            provider_binding_sha256,
            patch,
        } => Ok(to_value(
            supervisor
                .apply_repository_patch(
                    &execution_id,
                    &repository,
                    &base_sha,
                    &provider_binding_sha256,
                    patch,
                )
                .await?,
        )?),
        ExecutorCommand::ProcessStart {
            execution_id,
            argv,
            cwd,
            authorization_digest,
        } => Ok(to_value(
            supervisor
                .start_process(&execution_id, argv, cwd, authorization_digest)
                .await?,
        )?),
        ExecutorCommand::ProcessRead {
            execution_id,
            process_id,
            offset,
            limit,
        } => Ok(to_value(
            supervisor
                .read_process(&execution_id, &process_id, offset, limit)
                .await?,
        )?),
        ExecutorCommand::ProcessWait {
            execution_id,
            process_id,
            timeout_seconds,
        } => {
            let timeout = match timeout_seconds {
                None => None,
                Some(value) if value.is_finite() && value >= 0.0 && value <= MAX_WAIT_SECONDS => {
                    Some(Duration::from_secs_f64(value))
                }
                Some(_) => bail!("timeout_seconds must be between 0 and {MAX_WAIT_SECONDS}"),
            };
            Ok(to_value(
                supervisor
                    .wait_process(&execution_id, &process_id, timeout)
                    .await?,
            )?)
        }
        ExecutorCommand::ProcessAbort {
            execution_id,
            process_id,
        } => Ok(to_value(
            supervisor.abort_process(&execution_id, &process_id).await?,
        )?),
        ExecutorCommand::SessionClose { execution_id } => {
            Ok(to_value(supervisor.close_session(&execution_id).await?)?)
        }
        ExecutorCommand::UpgradePrepare {
            execution_id,
            attempt_id,
            expected_successor_revision,
            checkpoint_generation,
        } => Ok(to_value(
            supervisor
                .prepare_transactional_upgrade(
                    &execution_id,
                    &attempt_id,
                    &expected_successor_revision,
                    checkpoint_generation,
                )
                .await?,
        )?),
        ExecutorCommand::UpgradeInspect => {
            Ok(to_value(supervisor.inspect_transactional_upgrade())?)
        }
        ExecutorCommand::UpgradeResume {
            intent,
            external_event_payload,
        } => Ok(to_value(supervisor.resume_transactional_upgrade(
            intent,
            &external_event_payload,
        )?)?),
        ExecutorCommand::Health => {
            let recovery = supervisor.recovery_summary();
            Ok(json!({
                "protocol": EXECUTOR_PROTOCOL_VERSION,
                "status": "ready",
                "service": {"state": "SERVING"},
                "recovery": {
                    "state": recovery.health,
                    "runnable": recovery.runnable,
                    "quarantined": recovery.quarantined,
                    "failed_recovery": recovery.failed_recovery
                }
            }))
        }
    }
}
