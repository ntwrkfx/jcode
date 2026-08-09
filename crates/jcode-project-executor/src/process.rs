use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use uuid::Uuid;

const MAX_READ_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionProcessState {
    Running,
    Exited { code: Option<i32> },
    Aborted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionProcessRef {
    pub process_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionProcessOutput {
    pub process_id: String,
    pub offset: u64,
    pub next_offset: u64,
    pub data: String,
    pub eof: bool,
}

struct ProcessRecord {
    pid: u32,
    child: Mutex<Child>,
    output_path: PathBuf,
    state: Mutex<ExecutionProcessState>,
}

impl Drop for ProcessRecord {
    fn drop(&mut self) {
        kill_process_group(self.pid);
    }
}

pub struct ExecutionProcessManager {
    workspace: PathBuf,
    state_root: PathBuf,
    processes: Mutex<HashMap<String, Arc<ProcessRecord>>>,
}

impl ExecutionProcessManager {
    pub fn create(workspace: impl AsRef<Path>) -> Result<Self> {
        let state_root = std::env::temp_dir().join(format!(
            "jcode-project-executor-{}",
            Uuid::new_v4().simple()
        ));
        Self::create_with_state_root(workspace, state_root)
    }

    pub fn create_with_state_root(
        workspace: impl AsRef<Path>,
        state_root: impl AsRef<Path>,
    ) -> Result<Self> {
        let workspace = workspace
            .as_ref()
            .canonicalize()
            .context("canonicalize executor workspace")?;
        if !workspace.is_dir() {
            bail!("executor workspace must be a directory");
        }
        let state_root = state_root.as_ref().to_path_buf();
        std::fs::create_dir_all(state_root.join("processes"))
            .context("create executor process state directory")?;
        Ok(Self {
            workspace,
            state_root,
            processes: Mutex::new(HashMap::new()),
        })
    }

    pub async fn start(
        &self,
        argv: Vec<String>,
        cwd: Option<String>,
    ) -> Result<ExecutionProcessRef> {
        if argv.is_empty() || argv.iter().any(String::is_empty) {
            bail!("argv must be a non-empty array of non-empty strings");
        }
        let cwd = self.resolve_cwd(cwd.as_deref())?;
        let process_id = format!("p-{}", Uuid::new_v4().simple());
        let output_path = self
            .state_root
            .join("processes")
            .join(format!("{process_id}.log"));
        let output = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&output_path)
            .context("create process output file")?;
        let stderr = output.try_clone().context("clone process output handle")?;

        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]);
        command.current_dir(cwd);
        command.stdin(Stdio::null());
        command.stdout(Stdio::from(output));
        command.stderr(Stdio::from(stderr));
        command.kill_on_drop(true);
        #[cfg(unix)]
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let child = command.spawn().context("start owned process")?;
        let pid = child
            .id()
            .ok_or_else(|| anyhow!("process started without pid"))?;
        let record = Arc::new(ProcessRecord {
            pid,
            child: Mutex::new(child),
            output_path,
            state: Mutex::new(ExecutionProcessState::Running),
        });
        self.processes
            .lock()
            .await
            .insert(process_id.clone(), record);
        Ok(ExecutionProcessRef { process_id })
    }

    pub async fn read(
        &self,
        process_id: &str,
        offset: u64,
        limit: usize,
    ) -> Result<ExecutionProcessOutput> {
        if limit == 0 || limit > MAX_READ_BYTES {
            bail!("read limit must be between 1 and {MAX_READ_BYTES}");
        }
        let record = self.record(process_id).await?;
        let state = self.refresh(&record).await?;
        let mut file = tokio::fs::File::open(&record.output_path).await?;
        let len = file.metadata().await?.len();
        if offset > len {
            bail!("read offset exceeds available output");
        }
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        let remaining = len.saturating_sub(offset) as usize;
        let count = remaining.min(limit);
        let mut bytes = vec![0; count];
        if count > 0 {
            file.read_exact(&mut bytes).await?;
        }
        let next_offset = offset + count as u64;
        Ok(ExecutionProcessOutput {
            process_id: process_id.to_owned(),
            offset,
            next_offset,
            data: String::from_utf8_lossy(&bytes).into_owned(),
            eof: state != ExecutionProcessState::Running && next_offset >= len,
        })
    }

    pub async fn wait(
        &self,
        process_id: &str,
        timeout: Option<Duration>,
    ) -> Result<ExecutionProcessState> {
        let record = self.record(process_id).await?;
        let state = self.refresh(&record).await?;
        if state != ExecutionProcessState::Running {
            return Ok(state);
        }
        let mut child = record.child.lock().await;
        let status = match timeout {
            Some(duration) => tokio::time::timeout(duration, child.wait())
                .await
                .map_err(|_| anyhow!("process wait timed out"))??,
            None => child.wait().await?,
        };
        kill_process_group(record.pid);
        let state = ExecutionProcessState::Exited {
            code: status.code(),
        };
        *record.state.lock().await = state.clone();
        Ok(state)
    }

    pub async fn abort(&self, process_id: &str) -> Result<ExecutionProcessState> {
        let record = self.record(process_id).await?;
        let state = self.refresh(&record).await?;
        if state != ExecutionProcessState::Running {
            return Ok(state);
        }
        kill_process_group(record.pid);
        let mut child = record.child.lock().await;
        let _ = child.kill().await;
        let _ = child.wait().await;
        let state = ExecutionProcessState::Aborted;
        *record.state.lock().await = state.clone();
        Ok(state)
    }

    pub async fn close_all(&self) -> Result<()> {
        let ids: Vec<String> = self.processes.lock().await.keys().cloned().collect();
        for process_id in ids {
            self.abort(&process_id).await?;
        }
        Ok(())
    }

    async fn record(&self, process_id: &str) -> Result<Arc<ProcessRecord>> {
        self.processes
            .lock()
            .await
            .get(process_id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown process: {process_id}"))
    }

    async fn refresh(&self, record: &Arc<ProcessRecord>) -> Result<ExecutionProcessState> {
        let current = record.state.lock().await.clone();
        if current != ExecutionProcessState::Running {
            return Ok(current);
        }
        let mut child = record.child.lock().await;
        let Some(status) = child.try_wait()? else {
            return Ok(ExecutionProcessState::Running);
        };
        kill_process_group(record.pid);
        let state = ExecutionProcessState::Exited {
            code: status.code(),
        };
        *record.state.lock().await = state.clone();
        Ok(state)
    }

    fn resolve_cwd(&self, cwd: Option<&str>) -> Result<PathBuf> {
        let candidate = match cwd {
            None => self.workspace.clone(),
            Some(value) if Path::new(value).is_absolute() => PathBuf::from(value),
            Some(value) => self.workspace.join(value),
        };
        let resolved = candidate
            .canonicalize()
            .context("canonicalize process cwd")?;
        if !resolved.is_dir() {
            bail!("process cwd must be a directory");
        }
        if !resolved.starts_with(&self.workspace) {
            bail!("process cwd is outside workspace");
        }
        Ok(resolved)
    }
}

#[cfg(unix)]
fn kill_process_group(pid: u32) {
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_process_group(_pid: u32) {}
