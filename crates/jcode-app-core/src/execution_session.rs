use crate::id::new_id;
use crate::tool::{Registry, ToolContext, ToolExecutionMode, ToolOutput};
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionSessionInfo {
    pub id: String,
    pub workspace: PathBuf,
    pub closed: bool,
}

pub struct ExecutionSession {
    id: String,
    workspace: PathBuf,
    registry: Registry,
    closed: AtomicBool,
    next_call_id: AtomicU64,
}

impl ExecutionSession {
    pub async fn create(workspace: impl AsRef<Path>) -> Result<Self> {
        let workspace = workspace.as_ref();
        if !workspace.is_dir() {
            bail!("execution workspace must be an existing directory: {}", workspace.display());
        }
        let workspace = workspace.canonicalize()?;
        Ok(Self {            id: new_id("execution"),
            workspace,
            registry: Registry::execution().await,
            closed: AtomicBool::new(false),
            next_call_id: AtomicU64::new(1),
        })
    }

    pub fn inspect(&self) -> ExecutionSessionInfo {
        ExecutionSessionInfo {
            id: self.id.clone(),
            workspace: self.workspace.clone(),
            closed: self.closed.load(Ordering::Acquire),
        }
    }

    pub async fn tool_names(&self) -> Vec<String> {
        self.registry.tool_names().await
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    pub async fn call_tool(
        &self,
        name: &str,
        input: serde_json::Value,
    ) -> Result<ToolOutput> {        if self.closed.load(Ordering::Acquire) {
            bail!("execution session is closed");
        }
        if name == "bash"
            && input
                .get("run_in_background")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        {
            bail!("background bash requires a session-owned process plane");
        }
        let call_id = self.next_call_id.fetch_add(1, Ordering::Relaxed);
        let ctx = ToolContext {
            session_id: self.id.clone(),
            message_id: format!("execution:{}", self.id),
            tool_call_id: format!("execution-{}-{call_id}", self.id),
            working_dir: Some(self.workspace.clone()),
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: ToolExecutionMode::ExecutionSession,
        };
        self.registry.execute(name, input, ctx).await
    }
}
