use crate::process::{
    ExecutionProcessManager, ExecutionProcessOutput, ExecutionProcessRef, ExecutionProcessState,
};
use crate::session::{
    AccessMode, IMPLEMENTATION_NAME, PROVIDER_NAME, SESSION_SCHEMA_VERSION, SessionCreateRequest,
    SessionInspection, SessionRecord, SessionState, WORKTREE_BINDING_SCHEMA_VERSION,
    WORKTREE_INSPECTION_SCHEMA_VERSION, WorktreeBinding, WorktreeInspection, WorktreeMode,
    WorktreeOwnership,
};
use anyhow::{Context, Result, anyhow, bail};
use std::collections::{BTreeSet, HashMap};
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use uuid::Uuid;

struct WorktreeWriteLock {
    _file: File,
}

impl WorktreeWriteLock {
    fn acquire(lock_root: &Path, worktree: &Path) -> Result<Self> {
        std::fs::create_dir_all(lock_root).context("create worktree lock root")?;
        let key = stable_path_key(worktree);
        let lock_path = lock_root.join(format!("{key:016x}.lock"));
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)
            .context("open worktree lock")?;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                bail!("WORKTREE_BUSY: {}", worktree.display());
            }
            return Err(error).context("acquire worktree lock");
        }
        Ok(Self { _file: file })
    }
}

pub struct ProjectExecutorSupervisor {
    session_root: PathBuf,
    worktree_root: PathBuf,
    lock_root: PathBuf,
    implementation_revision: String,
    device_id: Option<String>,
    records: Mutex<HashMap<String, SessionRecord>>,
    managers: Mutex<HashMap<String, Arc<ExecutionProcessManager>>>,
    locks: Mutex<HashMap<String, WorktreeWriteLock>>,
}

impl ProjectExecutorSupervisor {
    pub fn create(
        session_root: impl AsRef<Path>,
        implementation_revision: impl Into<String>,
    ) -> Result<Self> {
        let session_root = session_root.as_ref().to_path_buf();
        let worktree_root = session_root
            .parent()
            .unwrap_or_else(|| Path::new("/"))
            .to_path_buf();
        Self::create_with_worktree_root(session_root, worktree_root, implementation_revision)
    }

    pub fn create_with_worktree_root_and_device_id(
        session_root: impl AsRef<Path>,
        worktree_root: impl AsRef<Path>,
        implementation_revision: impl Into<String>,
        device_id: impl Into<String>,
    ) -> Result<Self> {
        let device_id = Uuid::parse_str(&device_id.into())
            .context("invalid device_id")?
            .to_string();
        Self::create_with_worktree_root_inner(
            session_root,
            worktree_root,
            implementation_revision,
            Some(device_id),
        )
    }

    pub fn create_with_worktree_root(
        session_root: impl AsRef<Path>,
        worktree_root: impl AsRef<Path>,
        implementation_revision: impl Into<String>,
    ) -> Result<Self> {
        Self::create_with_worktree_root_inner(
            session_root,
            worktree_root,
            implementation_revision,
            None,
        )
    }

    fn create_with_worktree_root_inner(
        session_root: impl AsRef<Path>,
        worktree_root: impl AsRef<Path>,
        implementation_revision: impl Into<String>,
        device_id: Option<String>,
    ) -> Result<Self> {
        let session_root = session_root.as_ref().to_path_buf();
        std::fs::create_dir_all(&session_root).context("create project executor session root")?;
        let worktree_root = worktree_root
            .as_ref()
            .canonicalize()
            .context("canonicalize worktree root")?;
        if !worktree_root.is_dir() {
            bail!("worktree root must be a directory");
        }
        let lock_root = session_root.join(".worktree-locks");
        std::fs::create_dir_all(&lock_root).context("create project executor lock root")?;
        let implementation_revision = implementation_revision.into();
        validate_sha(&implementation_revision, "implementation_revision")?;
        let mut records = HashMap::new();
        let mut managers = HashMap::new();
        let mut locks = HashMap::new();
        for entry in
            std::fs::read_dir(&session_root).context("read project executor session root")?
        {
            let entry = entry?;
            if !entry.file_type()?.is_dir() || entry.file_name() == ".worktree-locks" {
                continue;
            }
            let record_path = entry.path().join("session.json");
            if !record_path.is_file() {
                continue;
            }
            let mut record: SessionRecord = serde_json::from_str(
                &std::fs::read_to_string(&record_path).context("read session record")?,
            )
            .context("parse session record")?;
            if record.schema_version != SESSION_SCHEMA_VERSION {
                bail!("unsupported session schema: {}", record.schema_version);
            }
            if record.worktree_binding.is_none() {
                record.worktree_binding = Some(legacy_managed_binding(&record));
                write_record(&entry.path(), &record)?;
            }
            if record.state == SessionState::Ready {
                if record.implementation_revision != implementation_revision {
                    records.insert(record.execution_id.clone(), record);
                    continue;
                }
                let binding = record_binding(&record)?;
                let workspace = PathBuf::from(&record.workspace);
                if !workspace.is_dir() {
                    record_recovery_quarantine(
                        &entry.path(),
                        &mut record,
                        "READY_WORKSPACE_MISSING",
                    )?;
                    records.insert(record.execution_id.clone(), record);
                    continue;
                }
                if binding.access_mode == AccessMode::Write {
                    locks.insert(
                        record.execution_id.clone(),
                        WorktreeWriteLock::acquire(&lock_root, &workspace)?,
                    );
                }
                let manager = ExecutionProcessManager::create_with_state_root_and_access(
                    &workspace,
                    entry.path().join("evidence"),
                    binding.access_mode == AccessMode::Write,
                )?;
                managers.insert(record.execution_id.clone(), Arc::new(manager));
            }
            records.insert(record.execution_id.clone(), record);
        }
        Ok(Self {
            session_root,
            worktree_root,
            lock_root,
            implementation_revision,
            device_id,
            records: Mutex::new(records),
            managers: Mutex::new(managers),
            locks: Mutex::new(locks),
        })
    }

    pub async fn list_worktrees(&self) -> Result<Vec<WorktreeInspection>> {
        let mut results = Vec::new();
        for path in discover_worktree_paths(&self.worktree_root)? {
            if let Ok(item) = self.inspect_worktree_path(&path).await {
                results.push(item);
            }
        }
        results.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(results)
    }

    pub async fn inspect_worktree(&self, path: &str) -> Result<WorktreeInspection> {
        if path.is_empty() {
            bail!("worktree path must not be empty");
        }
        let path = PathBuf::from(path)
            .canonicalize()
            .context("canonicalize worktree path")?;
        self.inspect_worktree_path(&path).await
    }

    async fn inspect_worktree_path(&self, path: &Path) -> Result<WorktreeInspection> {
        ensure_within_root(path, &self.worktree_root)?;
        let top = PathBuf::from(git(path, &["rev-parse", "--show-toplevel"])?).canonicalize()?;
        if top != path {
            bail!("worktree path is not an exact git toplevel");
        }
        let origin = git_optional(path, &["remote", "get-url", "origin"]);
        let repository = origin
            .as_deref()
            .and_then(canonical_repository_from_origin)
            .unwrap_or_else(|| origin.clone().unwrap_or_else(|| path.display().to_string()));
        let head_sha = git(path, &["rev-parse", "HEAD"])?;
        validate_sha(&head_sha, "worktree HEAD")?;
        let branch = branch(path)?;
        let dirty = !git(path, &["status", "--porcelain"])?.is_empty();
        let owner = command_output("stat", &["-c", "%U", &path.display().to_string()])?;
        let writer = self.writer_for_path(path).await?;
        Ok(WorktreeInspection {
            schema_version: WORKTREE_INSPECTION_SCHEMA_VERSION.to_owned(),
            path: path.display().to_string(),
            repository,
            branch,
            head_sha,
            origin,
            dirty,
            owner,
            writer,
        })
    }

    async fn writer_for_path(&self, path: &Path) -> Result<Option<String>> {
        let active_writers = self.locks.lock().await.keys().cloned().collect::<Vec<_>>();
        for record in self.records.lock().await.values() {
            if record.state != SessionState::Ready || !active_writers.contains(&record.execution_id)
            {
                continue;
            }
            let binding = record_binding(record)?;
            if binding.access_mode != AccessMode::Write {
                continue;
            }
            let candidate = PathBuf::from(&binding.path);
            if candidate.canonicalize().ok().as_deref() == Some(path) {
                return Ok(Some(record.execution_id.clone()));
            }
        }
        Ok(None)
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

        let session_dir = self.session_root.join(&request.execution_id);
        let evidence = session_dir.join("evidence");

        let (workspace, session_branch, binding, managed_created) = match request
            .worktree_path
            .as_deref()
        {
            Some(raw_path) => {
                let access_mode = request.access_mode.ok_or_else(|| {
                    anyhow!("access_mode is required for existing worktree binding")
                })?;
                let workspace = PathBuf::from(raw_path)
                    .canonicalize()
                    .context("canonicalize existing worktree")?;
                ensure_within_root(&workspace, &self.worktree_root)?;
                let top = PathBuf::from(git(&workspace, &["rev-parse", "--show-toplevel"])?)
                    .canonicalize()?;
                if top != workspace {
                    let _ = std::fs::remove_dir(&session_dir);
                    bail!("worktree_path is not an exact git toplevel");
                }
                if !same_repository(&repository_path, &workspace)? {
                    let _ = std::fs::remove_dir(&session_dir);
                    bail!("worktree_path does not belong to admitted repository");
                }
                let head_sha = git(&workspace, &["rev-parse", "HEAD"])?;
                if head_sha != request.base_sha {
                    let _ = std::fs::remove_dir(&session_dir);
                    bail!("existing worktree HEAD does not match base_sha");
                }
                let session_branch = branch(&workspace)?;
                let binding = WorktreeBinding {
                    schema_version: WORKTREE_BINDING_SCHEMA_VERSION.to_owned(),
                    mode: WorktreeMode::Existing,
                    ownership: WorktreeOwnership::External,
                    path: workspace.display().to_string(),
                    repository: request.repository.clone(),
                    resolved_sha: request.base_sha.clone(),
                    access_mode,
                    device_id: None,
                    git_common_dir: None,
                };
                std::fs::create_dir(&session_dir).context("create executor session directory")?;
                (workspace, session_branch, binding, false)
            }
            None => {
                if request.access_mode.is_some() {
                    bail!("access_mode requires worktree_path");
                }
                std::fs::create_dir(&session_dir).context("create executor session directory")?;
                let workspace = session_dir.join("workspace");
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
                let binding = WorktreeBinding {
                    schema_version: WORKTREE_BINDING_SCHEMA_VERSION.to_owned(),
                    mode: WorktreeMode::Managed,
                    ownership: WorktreeOwnership::Harness,
                    path: workspace.display().to_string(),
                    repository: request.repository.clone(),
                    resolved_sha: request.base_sha.clone(),
                    access_mode: AccessMode::Write,
                    device_id: None,
                    git_common_dir: None,
                };
                (workspace, branch(&repository_path)?, binding, true)
            }
        };

        std::fs::create_dir(&evidence).context("create executor evidence directory")?;
        let write_lock = if binding.access_mode == AccessMode::Write {
            match WorktreeWriteLock::acquire(&self.lock_root, &workspace) {
                Ok(lock) => Some(lock),
                Err(error) => {
                    if managed_created {
                        let _ = remove_managed_worktree(&repository_path, &workspace);
                    }
                    let _ = std::fs::remove_dir_all(&session_dir);
                    return Err(error);
                }
            }
        } else {
            None
        };
        let manager = match ExecutionProcessManager::create_with_state_root_and_access(
            &workspace,
            &evidence,
            binding.access_mode == AccessMode::Write,
        ) {
            Ok(manager) => Arc::new(manager),
            Err(error) => {
                drop(write_lock);
                if managed_created {
                    let _ = remove_managed_worktree(&repository_path, &workspace);
                }
                let _ = std::fs::remove_dir_all(&session_dir);
                return Err(error);
            }
        };
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
            branch: session_branch,
            workspace: workspace.display().to_string(),
            worktree_binding: Some(binding),
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
        if let Some(lock) = write_lock {
            self.locks
                .lock()
                .await
                .insert(record.execution_id.clone(), lock);
        }
        self.inspect_session(&record.execution_id).await
    }

    pub async fn inspect_session(&self, execution_id: &str) -> Result<SessionInspection> {
        let record = self.record(execution_id).await?;
        let mut binding = record_binding(&record)?;
        let (head_sha, clean) = if record.state == SessionState::Ready {
            let workspace = PathBuf::from(&record.workspace);
            if !workspace.is_dir() {
                bail!("session workspace is missing: {}", workspace.display());
            }
            if let Some(device_id) = &self.device_id {
                binding.device_id = Some(device_id.clone());
                binding.git_common_dir = Some(git_common_dir(&workspace)?.display().to_string());
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
            worktree_binding: binding,
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
        let binding = record_binding(&record)?;
        let workspace = PathBuf::from(&record.workspace);
        if binding.mode == WorktreeMode::Managed
            && binding.ownership == WorktreeOwnership::Harness
            && workspace.exists()
        {
            remove_managed_worktree(Path::new(&record.repository_path), &workspace)?;
        }
        self.locks.lock().await.remove(execution_id);
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

fn record_binding(record: &SessionRecord) -> Result<WorktreeBinding> {
    Ok(record
        .worktree_binding
        .clone()
        .unwrap_or_else(|| legacy_managed_binding(record)))
}

fn legacy_managed_binding(record: &SessionRecord) -> WorktreeBinding {
    WorktreeBinding {
        schema_version: WORKTREE_BINDING_SCHEMA_VERSION.to_owned(),
        mode: WorktreeMode::Managed,
        ownership: WorktreeOwnership::Harness,
        path: record.workspace.clone(),
        repository: record.repository.clone(),
        resolved_sha: record.base_sha.clone(),
        access_mode: AccessMode::Write,
        device_id: None,
        git_common_dir: None,
    }
}

fn remove_managed_worktree(repository: &Path, workspace: &Path) -> Result<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["worktree", "remove"])
        .arg(workspace)
        .output()
        .context("start git worktree remove")?;
    if !output.status.success() {
        bail!(
            "git worktree remove failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn discover_worktree_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let output = Command::new("find")
        .arg(root)
        .args([
            "(",
            "-type",
            "d",
            "(",
            "-name",
            "node_modules",
            "-o",
            "-name",
            ".cache",
            "-o",
            "-name",
            ".npm",
            "-o",
            "-name",
            ".pnpm-store",
            "-o",
            "-name",
            ".venv",
            "-o",
            "-name",
            "target",
            ")",
            "-prune",
            ")",
            "-o",
            "(",
            "-name",
            ".git",
            "(",
            "-type",
            "d",
            "-o",
            "-type",
            "f",
            ")",
            "-print",
            ")",
        ])
        .output()
        .context("discover git worktree markers")?;
    if !output.status.success() {
        bail!(
            "worktree discovery failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let mut paths = BTreeSet::new();
    for line in String::from_utf8(output.stdout)?.lines() {
        let marker = PathBuf::from(line);
        let Some(parent) = marker.parent() else {
            continue;
        };
        let Ok(parent) = parent.canonicalize() else {
            continue;
        };
        let Ok(top) = git(&parent, &["rev-parse", "--show-toplevel"]) else {
            continue;
        };
        let Ok(top) = PathBuf::from(top).canonicalize() else {
            continue;
        };
        if top == parent && top.starts_with(root) {
            paths.insert(top);
        }
    }
    Ok(paths.into_iter().collect())
}

fn ensure_within_root(path: &Path, root: &Path) -> Result<()> {
    if !path.starts_with(root) {
        bail!("worktree path is outside admitted discovery root");
    }
    Ok(())
}

fn git_common_dir(worktree: &Path) -> Result<PathBuf> {
    let raw = PathBuf::from(git(worktree, &["rev-parse", "--git-common-dir"])?);
    let candidate = if raw.is_absolute() {
        raw
    } else {
        worktree.join(raw)
    };
    candidate
        .canonicalize()
        .context("canonicalize git common directory")
}

fn same_repository(admitted: &Path, worktree: &Path) -> Result<bool> {
    if git_common_dir(admitted)? == git_common_dir(worktree)? {
        return Ok(true);
    }
    let admitted_origin = git_optional(admitted, &["remote", "get-url", "origin"]);
    let worktree_origin = git_optional(worktree, &["remote", "get-url", "origin"]);
    let (Some(admitted_origin), Some(worktree_origin)) = (admitted_origin, worktree_origin) else {
        return Ok(false);
    };
    if admitted_origin.trim_end_matches(".git") == worktree_origin.trim_end_matches(".git") {
        return Ok(true);
    }
    Ok(canonical_repository_from_origin(&admitted_origin).is_some()
        && canonical_repository_from_origin(&admitted_origin)
            == canonical_repository_from_origin(&worktree_origin))
}

fn canonical_repository_from_origin(origin: &str) -> Option<String> {
    let value = origin.strip_suffix(".git").unwrap_or(origin);
    for prefix in [
        "https://github.com/",
        "ssh://git@github.com/",
        "git@github.com:",
    ] {
        if let Some(repository) = value.strip_prefix(prefix) {
            let mut parts = repository.split('/');
            let owner = parts.next()?;
            let name = parts.next()?;
            if parts.next().is_none() && !owner.is_empty() && !name.is_empty() {
                return Some(format!("{owner}/{name}"));
            }
        }
    }
    None
}

fn stable_path_key(path: &Path) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
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

fn record_recovery_quarantine(
    session_dir: &Path,
    record: &mut SessionRecord,
    reason: &str,
) -> Result<()> {
    record.state = SessionState::Failed;
    write_record(session_dir, record)?;
    let evidence_dir = session_dir.join("evidence");
    std::fs::create_dir_all(&evidence_dir).context("create recovery evidence directory")?;
    let target = evidence_dir.join("recovery-quarantine.json");
    let temporary = evidence_dir.join("recovery-quarantine.json.tmp");
    let payload = serde_json::json!({
        "schema_version": "project-executor-recovery-quarantine/v1",
        "execution_id": record.execution_id,
        "implementation_revision": record.implementation_revision,
        "workspace": record.workspace,
        "reason": reason,
        "state": "FAILED",
    });
    std::fs::write(&temporary, serde_json::to_vec_pretty(&payload)?)
        .context("write recovery quarantine evidence")?;
    std::fs::rename(&temporary, &target).context("commit recovery quarantine evidence")?;
    Ok(())
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

fn git_optional(repository: &Path, args: &[&str]) -> Option<String> {
    git(repository, args).ok().filter(|value| !value.is_empty())
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

fn command_output(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("start {program}"))?;
    if !output.status.success() {
        bail!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}
