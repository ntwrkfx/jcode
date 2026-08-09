use crate::process::{
    ExecutionProcessManager, ExecutionProcessOutput, ExecutionProcessRef, ExecutionProcessState,
};
use crate::session::{
    IMPLEMENTATION_NAME, PROVIDER_NAME, SESSION_SCHEMA_VERSION, SessionCreateRequest,
    SessionInspection, SessionRecord, SessionState,
};
use anyhow::{Context, Result, anyhow, bail};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use uuid::Uuid;

pub struct ProjectExecutorSupervisor {
    session_root: PathBuf,
    implementation_revision: String,
    records: Mutex<HashMap<String, SessionRecord>>,
    managers: Mutex<HashMap<String, Arc<ExecutionProcessManager>>>,
}

impl ProjectExecutorSupervisor {
    pub fn create(
        session_root: impl AsRef<Path>,
        implementation_revision: impl Into<String>,
    ) -> Result<Self> {
        let session_root = session_root.as_ref().to_path_buf();
        std::fs::create_dir_all(&session_root).context("create project executor session root")?;
        let implementation_revision = implementation_revision.into();
        validate_sha(&implementation_revision, "implementation_revision")?;
        let mut records = HashMap::new();
        let mut managers = HashMap::new();
        for entry in
            std::fs::read_dir(&session_root).context("read project executor session root")?
        {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let record_path = entry.path().join("session.json");
            if !record_path.is_file() {
                continue;
            }
            let record: SessionRecord = serde_json::from_str(
                &std::fs::read_to_string(&record_path).context("read session record")?,
            )
            .context("parse session record")?;
            if record.schema_version != SESSION_SCHEMA_VERSION {
                bail!("unsupported session schema: {}", record.schema_version);
            }
            if record.state == SessionState::Ready {
                if record.implementation_revision != implementation_revision {
                    bail!(
                        "ready session {} belongs to executor revision {}",
                        record.execution_id,
                        record.implementation_revision
                    );
                }
                let workspace = PathBuf::from(&record.workspace);
                if !workspace.is_dir() {
                    bail!(
                        "ready session workspace is missing: {}",
                        workspace.display()
                    );
                }
                let manager = ExecutionProcessManager::create_with_state_root(
                    &workspace,
                    entry.path().join("evidence"),
                )?;
                managers.insert(record.execution_id.clone(), Arc::new(manager));
            }
            records.insert(record.execution_id.clone(), record);
        }
        Ok(Self {
            session_root,
            implementation_revision,
            records: Mutex::new(records),
            managers: Mutex::new(managers),
        })
    }

    pub async fn create_session(&self, request: SessionCreateRequest) -> Result<SessionInspection> {
        Uuid::parse_str(&request.execution_id).context("execution_id must be a UUID")?;
        if request.work_identity.is_empty() {
            bail!("work_identity must not be empty");
        }
        validate_sha(&request.base_sha, "base_sha")?;
        if self
            .records
            .lock()
            .await
            .contains_key(&request.execution_id)
        {
            bail!("execution already exists: {}", request.execution_id);
        }

        let requested_repo = PathBuf::from(&request.repository);
        let repository_path =
            PathBuf::from(git(&requested_repo, &["rev-parse", "--show-toplevel"])?);
        let repository_path = repository_path
            .canonicalize()
            .context("canonicalize repository")?;
        let resolved_sha = git(
            &repository_path,
            &["rev-parse", &format!("{}^{{commit}}", request.base_sha)],
        )?;
        if resolved_sha != request.base_sha {
            bail!("base_sha does not resolve exactly to requested commit");
        }
        let branch = branch(&repository_path)?;
        let session_dir = self.session_root.join(&request.execution_id);
        let workspace = session_dir.join("workspace");
        let evidence = session_dir.join("evidence");
        std::fs::create_dir(&session_dir).context("create executor session directory")?;
        let add_result = Command::new("git")
            .arg("-C")
            .arg(&repository_path)
            .args(["worktree", "add", "--detach"])
            .arg(&workspace)
            .arg(&request.base_sha)
            .output()
            .context("start git worktree add")?;
        if !add_result.status.success() {
            let _ = std::fs::remove_dir(&session_dir);
            bail!(
                "git worktree add failed: {}",
                String::from_utf8_lossy(&add_result.stderr).trim()
            );
        }
        std::fs::create_dir(&evidence).context("create executor evidence directory")?;
        let manager = Arc::new(ExecutionProcessManager::create_with_state_root(
            &workspace, &evidence,
        )?);
        let record = SessionRecord {
            schema_version: SESSION_SCHEMA_VERSION.to_owned(),
            execution_id: request.execution_id.clone(),
            provider: PROVIDER_NAME.to_owned(),
            implementation: IMPLEMENTATION_NAME.to_owned(),
            implementation_revision: self.implementation_revision.clone(),
            work_identity: request.work_identity,
            repository: request.repository,
            repository_path: repository_path.display().to_string(),
            base_sha: request.base_sha,
            branch,
            workspace: workspace.display().to_string(),
            state: SessionState::Ready,
            created_at: now_millis()?,
            closed_at: None,
        };
        write_record(&session_dir, &record)?;
        self.records
            .lock()
            .await
            .insert(record.execution_id.clone(), record.clone());
        self.managers
            .lock()
            .await
            .insert(record.execution_id.clone(), manager);
        self.inspect_session(&record.execution_id).await
    }

    pub async fn inspect_session(&self, execution_id: &str) -> Result<SessionInspection> {
        let record = self.record(execution_id).await?;
        let (head_sha, clean) = if record.state == SessionState::Ready {
            let workspace = PathBuf::from(&record.workspace);
            if !workspace.is_dir() {
                bail!("session workspace is missing: {}", workspace.display());
            }
            (
                git(&workspace, &["rev-parse", "HEAD"])?,
                git(&workspace, &["status", "--porcelain"])?.is_empty(),
            )
        } else {
            (record.base_sha.clone(), true)
        };
        Ok(SessionInspection {
            schema_version: record.schema_version,
            execution_id: record.execution_id,
            provider: record.provider,
            implementation: record.implementation,
            implementation_revision: record.implementation_revision,
            work_identity: record.work_identity,
            repository: record.repository,
            repository_path: record.repository_path,
            base_sha: record.base_sha,
            branch: record.branch,
            workspace: record.workspace,
            state: record.state,
            created_at: record.created_at,
            closed_at: record.closed_at,
            head_sha,
            clean,
        })
    }

    pub async fn start_process(
        &self,
        execution_id: &str,
        argv: Vec<String>,
        cwd: Option<String>,
    ) -> Result<ExecutionProcessRef> {
        let manager = self.manager(execution_id).await?;
        let local = manager.start(argv, cwd).await?;
        Ok(ExecutionProcessRef {
            process_id: namespace_process_id(execution_id, &local.process_id),
        })
    }

    pub async fn read_process(
        &self,
        execution_id: &str,
        process_id: &str,
        offset: u64,
        limit: usize,
    ) -> Result<ExecutionProcessOutput> {
        let local_id = local_process_id(execution_id, process_id)?;
        let manager = self.manager(execution_id).await?;
        let mut output = manager.read(local_id, offset, limit).await?;
        output.process_id = process_id.to_owned();
        Ok(output)
    }

    pub async fn wait_process(
        &self,
        execution_id: &str,
        process_id: &str,
        timeout: Option<Duration>,
    ) -> Result<ExecutionProcessState> {
        let local_id = local_process_id(execution_id, process_id)?;
        self.manager(execution_id)
            .await?
            .wait(local_id, timeout)
            .await
    }

    pub async fn abort_process(
        &self,
        execution_id: &str,
        process_id: &str,
    ) -> Result<ExecutionProcessState> {
        let local_id = local_process_id(execution_id, process_id)?;
        self.manager(execution_id).await?.abort(local_id).await
    }

    pub async fn close_session(&self, execution_id: &str) -> Result<SessionInspection> {
        let mut record = self.record(execution_id).await?;
        if record.state == SessionState::Closed {
            return self.inspect_session(execution_id).await;
        }
        if let Some(manager) = self.managers.lock().await.get(execution_id).cloned() {
            manager.close_all().await?;
        }
        let workspace = PathBuf::from(&record.workspace);
        if workspace.exists() {
            let output = Command::new("git")
                .arg("-C")
                .arg(&record.repository_path)
                .args(["worktree", "remove"])
                .arg(&workspace)
                .output()
                .context("start git worktree remove")?;
            if !output.status.success() {
                bail!(
                    "git worktree remove failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
        }
        record.state = SessionState::Closed;
        record.closed_at = Some(now_millis()?);
        write_record(&self.session_root.join(execution_id), &record)?;
        self.records
            .lock()
            .await
            .insert(execution_id.to_owned(), record);
        self.managers.lock().await.remove(execution_id);
        self.inspect_session(execution_id).await
    }

    async fn record(&self, execution_id: &str) -> Result<SessionRecord> {
        self.records
            .lock()
            .await
            .get(execution_id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown execution: {execution_id}"))
    }

    async fn manager(&self, execution_id: &str) -> Result<Arc<ExecutionProcessManager>> {
        let record = self.record(execution_id).await?;
        if record.state == SessionState::Closed {
            bail!("execution is closed: {execution_id}");
        }
        if record.state != SessionState::Ready {
            bail!("execution is not ready: {execution_id}");
        }
        self.managers
            .lock()
            .await
            .get(execution_id)
            .cloned()
            .ok_or_else(|| anyhow!("execution manager is unavailable: {execution_id}"))
    }
}

fn namespace_process_id(execution_id: &str, local_id: &str) -> String {
    format!("{execution_id}:{local_id}")
}

fn local_process_id<'a>(execution_id: &str, process_id: &'a str) -> Result<&'a str> {
    let prefix = format!("{execution_id}:");
    process_id
        .strip_prefix(&prefix)
        .ok_or_else(|| anyhow!("process does not belong to execution {execution_id}"))
}

fn validate_sha(value: &str, field: &str) -> Result<()> {
    if value.len() != 40 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("{field} must be exactly 40 hexadecimal characters");
    }
    Ok(())
}

fn now_millis() -> Result<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before unix epoch")?
        .as_millis();
    u64::try_from(millis).context("timestamp does not fit u64")
}

fn write_record(session_dir: &Path, record: &SessionRecord) -> Result<()> {
    let target = session_dir.join("session.json");
    let temporary = session_dir.join("session.json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(record)?)
        .context("write session record")?;
    std::fs::rename(&temporary, &target).context("commit session record")?;
    Ok(())
}

fn branch(repository: &Path) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["symbolic-ref", "--short", "-q", "HEAD"])
        .output()
        .context("resolve repository branch")?;
    match output.status.code() {
        Some(0) => Ok(String::from_utf8(output.stdout)?.trim().to_owned()),
        Some(1) => Ok("detached".to_owned()),
        _ => bail!(
            "unable to resolve repository branch: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

fn git(repository: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .output()
        .with_context(|| format!("start git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}
