use crate::process::ExecutionProcessManager;
use crate::protocol::{EXECUTOR_PROTOCOL_VERSION, WorkerCommand, WorkerRequest, WorkerResponse};
use anyhow::{Result, bail};
use serde_json::{json, to_value};
use std::path::Path;
use std::time::Duration;

const MAX_WAIT_SECONDS: f64 = 600.0;

pub struct ExecutionWorker {
    manager: ExecutionProcessManager,
}

impl ExecutionWorker {
    pub fn create(workspace: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            manager: ExecutionProcessManager::create(workspace)?,
        })
    }

    pub fn create_with_state_root(
        workspace: impl AsRef<Path>,
        state_root: impl AsRef<Path>,
    ) -> Result<Self> {
        Ok(Self {
            manager: ExecutionProcessManager::create_with_state_root(workspace, state_root)?,
        })
    }

    pub async fn handle(&self, request: WorkerRequest) -> WorkerResponse {
        let id = request.id.clone();
        match self.handle_inner(request).await {
            Ok(result) => WorkerResponse::success(id, result),
            Err(error) => WorkerResponse::failure(id, error.to_string()),
        }
    }

    async fn handle_inner(&self, request: WorkerRequest) -> Result<serde_json::Value> {
        if request.protocol != EXECUTOR_PROTOCOL_VERSION {
            bail!("unsupported executor protocol: {}", request.protocol);
        }
        if request.id.is_empty() {
            bail!("request id must not be empty");
        }
        match request.command {
            WorkerCommand::ProcessStart { argv, cwd } => {
                Ok(to_value(self.manager.start(argv, cwd).await?)?)
            }
            WorkerCommand::ProcessRead {
                process_id,
                offset,
                limit,
            } => Ok(to_value(
                self.manager.read(&process_id, offset, limit).await?,
            )?),
            WorkerCommand::ProcessWait {
                process_id,
                timeout_seconds,
            } => {
                let timeout = match timeout_seconds {
                    None => None,
                    Some(value)
                        if value.is_finite() && value >= 0.0 && value <= MAX_WAIT_SECONDS =>
                    {
                        Some(Duration::from_secs_f64(value))
                    }
                    Some(_) => bail!("timeout_seconds must be between 0 and {MAX_WAIT_SECONDS}"),
                };
                Ok(to_value(self.manager.wait(&process_id, timeout).await?)?)
            }
            WorkerCommand::ProcessAbort { process_id } => {
                Ok(to_value(self.manager.abort(&process_id).await?)?)
            }
            WorkerCommand::Close => {
                self.manager.close_all().await?;
                Ok(json!({"closed": true}))
            }
        }
    }
}
