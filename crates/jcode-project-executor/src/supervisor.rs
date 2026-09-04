use crate::custody::{
    CustodyAcquireRequest, CustodyEffectClaim, CustodyGrant, CustodyScope, LocalCustodyProvider,
};
use crate::durable::durable_write_json;
use crate::effect::{
    EffectAuthorizationBinding, MaterialAuthorizationClaim, WRITE_PROCESS_EFFECT_CLASS,
    validate_authorization_digest,
};
use crate::process::{
    ExecutionProcessManager, ExecutionProcessOutput, ExecutionProcessRef, ExecutionProcessState,
};
use crate::recovery::{
    RECOVERY_RECEIPT_SCHEMA_VERSION, RecoveryEvidence, RecoveryHealth, RecoveryReasonCode,
    RecoverySummary, SessionRecoveryReceipt, decode_session_record, derive_runnability,
    observe_git_material, summarize_recovery, unknown_custody,
};
use crate::retirement::{
    RETIREMENT_RECEIPT_SCHEMA_VERSION, RetirementIntent, RetirementOutcome, RetirementReasonCode,
    RetirementReceipt, WORKSPACE_RETIRE_EFFECT_CLASS,
};
use crate::session::{
    AccessMode, CustodyAssessment, CustodyState, ExpectedMaterial, IMPLEMENTATION_NAME,
    LocalMaterialState, PROVIDER_NAME, SESSION_SCHEMA_VERSION, SessionCreateRequest,
    SessionInspection, SessionLifecycle, SessionRecord, SessionRunnability,
    WORKTREE_BINDING_SCHEMA_VERSION, WORKTREE_INSPECTION_SCHEMA_VERSION, WorkspaceOrigin,
    WorktreeBinding, WorktreeInspection, WorktreeMode, WorktreeOwnership,
};
use crate::transactional_upgrade::{
    ResumeIntent, TransactionalUpgradeRequest, TransactionalUpgradeState,
    TransactionalUpgradeStore, UpgradePhase, material_identity, material_observation_identity,
    new_transaction_id,
};
use anyhow::{Context, Result, anyhow, bail};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

pub struct ProjectExecutorSupervisor {
    session_root: PathBuf,
    worktree_root: PathBuf,
    implementation_revision: String,
    device_id: Option<String>,
    executor_instance_id: String,
    recovery_summary: RecoverySummary,
    records: Mutex<HashMap<String, SessionRecord>>,
    managers: Mutex<HashMap<String, Arc<ExecutionProcessManager>>>,
    custody_provider: Mutex<LocalCustodyProvider>,
    custody_grants: Mutex<HashMap<String, CustodyGrant>>,
    upgrade_store: TransactionalUpgradeStore,
    upgrade_state: StdMutex<Option<TransactionalUpgradeState>>,
    admission_gate: RwLock<()>,
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
        let upgrade_store = TransactionalUpgradeStore::new(
            session_root
                .join(".transactional-upgrade")
                .join("state.json"),
        );
        let mut upgrade_state = if upgrade_store.path().is_file() {
            Some(upgrade_store.load()?)
        } else {
            None
        };
        let executor_instance_id = Uuid::new_v4().to_string();
        let mut records = HashMap::new();
        let mut managers = HashMap::new();
        let mut custody_provider =
            LocalCustodyProvider::new("PROJECT_EXECUTOR_LOCAL_FLOCK_V1", &lock_root)?;
        let mut custody_grants = HashMap::new();
        let mut receipts = Vec::new();
        let mut unidentified_failures = 0usize;
        let mut session_dirs = Vec::new();
        for entry in
            std::fs::read_dir(&session_root).context("read project executor session root")?
        {
            let entry = entry?;
            if !entry.file_type()?.is_dir() || entry.file_name() == ".worktree-locks" {
                continue;
            }
            if entry.path().join("session.json").is_file() {
                session_dirs.push(entry.path());
            }
        }
        session_dirs.sort();

        for session_dir in session_dirs {
            let record_path = session_dir.join("session.json");
            let raw: serde_json::Value = match serde_json::from_str(
                &std::fs::read_to_string(&record_path).context("read session record")?,
            ) {
                Ok(value) => value,
                Err(_) => {
                    unidentified_failures += 1;
                    continue;
                }
            };
            let decoded = match decode_session_record(raw) {
                Ok(value) => value,
                Err(_) => {
                    unidentified_failures += 1;
                    continue;
                }
            };
            let translated_from_legacy = decoded.translated_from_legacy;
            let schema_understood = decoded.schema_understood;
            let initial_reason_codes = decoded.initial_reason_codes;
            let mut record = decoded.record;
            if record.lifecycle != SessionLifecycle::Ready {
                records.insert(record.execution_id.clone(), record);
                continue;
            }

            let workspace = PathBuf::from(&record.workspace);
            let observed_material = observe_git_material(&workspace);
            if !schema_understood {
                record.runnability = SessionRunnability::Quarantined;
                record.custody_assessment = Some(unknown_custody());
                let receipt = recovery_receipt(
                    &record,
                    &implementation_revision,
                    workspace.is_dir().then(|| record.workspace.clone()),
                    observed_material,
                    record
                        .custody_assessment
                        .clone()
                        .unwrap_or_else(unknown_custody),
                    SessionRunnability::Quarantined,
                    initial_reason_codes,
                );
                write_recovery_receipt(&session_dir, &executor_instance_id, &receipt)?;
                receipts.push(receipt);
                records.insert(record.execution_id.clone(), record);
                continue;
            }
            if !workspace.is_dir() {
                record.runnability = SessionRunnability::FailedRecovery;
                record.custody_assessment = Some(not_held_custody());
                write_record(&session_dir, &record)?;
                let receipt = recovery_receipt(
                    &record,
                    &implementation_revision,
                    None,
                    observed_material,
                    record
                        .custody_assessment
                        .clone()
                        .unwrap_or_else(unknown_custody),
                    SessionRunnability::FailedRecovery,
                    vec![RecoveryReasonCode::ReadyWorkspaceMissing],
                );
                write_recovery_receipt(&session_dir, &executor_instance_id, &receipt)?;
                receipts.push(receipt);
                records.insert(record.execution_id.clone(), record);
                continue;
            }

            if translated_from_legacy {
                record.runnability = SessionRunnability::Quarantined;
                record.custody_assessment = Some(unknown_custody());
                let receipt = recovery_receipt(
                    &record,
                    &implementation_revision,
                    Some(record.workspace.clone()),
                    observed_material,
                    record
                        .custody_assessment
                        .clone()
                        .unwrap_or_else(unknown_custody),
                    SessionRunnability::Quarantined,
                    initial_reason_codes,
                );
                write_recovery_receipt(&session_dir, &executor_instance_id, &receipt)?;
                receipts.push(receipt);
                records.insert(record.execution_id.clone(), record);
                continue;
            }

            let binding = record_binding(&record)?;
            let binding_matches = exact_binding_matches(&binding, &workspace);
            let material_sufficiently_known = record.expected_material.local_material
                == LocalMaterialState::None
                && observed_material.local_material != LocalMaterialState::Unknown;
            let material_matches = observed_material.head_sha.as_deref()
                == Some(record.expected_material.head_sha.as_str())
                && observed_material.local_material == record.expected_material.local_material;
            let write_required = binding.access_mode == AccessMode::Write;
            let revision_matches = recovery_revision_matches(
                upgrade_state.as_ref(),
                &record,
                &implementation_revision,
            );
            let mut recovered_grant = None;
            let effect_authorization_valid = !write_required
                || record
                    .effect_authorization
                    .as_ref()
                    .map(|binding| {
                        validate_material_authorization(
                            &record,
                            &workspace,
                            Some(&binding.authorization_digest),
                        )
                        .is_ok()
                    })
                    .unwrap_or(false);
            let eligible_for_custody = record.lifecycle == SessionLifecycle::Ready
                && revision_matches
                && binding_matches
                && material_sufficiently_known
                && material_matches
                && effect_authorization_valid;
            let custody = if write_required && eligible_for_custody {
                match custody_provider.acquire(CustodyAcquireRequest {
                    scope: custody_scope(&record.execution_id, &workspace)?,
                    executor_instance_id: executor_instance_id.clone(),
                }) {
                    Ok(grant) => {
                        let assessment = custody_provider.assess(&grant);
                        recovered_grant = Some(grant);
                        assessment
                    }
                    Err(error) if error.to_string().contains("CUSTODY_NOT_CURRENT") => {
                        not_held_custody()
                    }
                    Err(_) => unknown_custody(),
                }
            } else {
                unknown_custody()
            };
            let evidence = RecoveryEvidence {
                lifecycle: record.lifecycle,
                workspace_origin: record.workspace_origin,
                schema_understood: true,
                revision_matches,
                binding_matches,
                material_matches,
                material_sufficiently_known,
                custody: custody.clone(),
                current_executor_instance_id: executor_instance_id.clone(),
                write_custody_required: write_required,
            };
            let mut decision = derive_runnability(&evidence);
            if write_required && custody.state == CustodyState::NotHeld {
                decision.reason_codes = vec![RecoveryReasonCode::CustodyContention];
            }
            if decision.result == SessionRunnability::Runnable {
                match ExecutionProcessManager::create_with_state_root_and_access(
                    &workspace,
                    session_dir.join("evidence"),
                    write_required,
                ) {
                    Ok(manager) => {
                        managers.insert(record.execution_id.clone(), Arc::new(manager));
                        if let Some(grant) = recovered_grant.take() {
                            custody_grants.insert(record.execution_id.clone(), grant);
                        }
                    }
                    Err(_) => {
                        if let Some(grant) = recovered_grant.take() {
                            let _ = custody_provider.release(&grant);
                        }
                        decision.result = SessionRunnability::Quarantined;
                        decision.reason_codes = vec![RecoveryReasonCode::RecoveryProbeFailed];
                    }
                }
            } else if let Some(grant) = recovered_grant.take() {
                let _ = custody_provider.release(&grant);
            }
            if decision.result == SessionRunnability::Runnable
                && pending_upgrade_targets(upgrade_state.as_ref(), &record.execution_id)
            {
                record.implementation_revision = implementation_revision.clone();
            }
            record.runnability = decision.result;
            record.custody_assessment = Some(custody.clone());
            write_record(&session_dir, &record)?;
            let receipt = recovery_receipt(
                &record,
                &implementation_revision,
                Some(record.workspace.clone()),
                observed_material,
                custody,
                decision.result,
                decision.reason_codes,
            );
            write_recovery_receipt(&session_dir, &executor_instance_id, &receipt)?;
            receipts.push(receipt);
            records.insert(record.execution_id.clone(), record);
        }
        let mut recovery_summary = summarize_recovery(receipts);
        if unidentified_failures > 0 {
            recovery_summary.failed_recovery += unidentified_failures;
            recovery_summary.health =
                if recovery_summary.runnable == 0 && recovery_summary.quarantined == 0 {
                    RecoveryHealth::Failed
                } else {
                    RecoveryHealth::Degraded
                };
        }
        if let Some(state) = upgrade_state.as_mut() {
            if state.phase != UpgradePhase::Complete && state.phase != UpgradePhase::Failed {
                let execution_id = state.request.execution_id.clone();
                let recovered = records
                    .get(&execution_id)
                    .filter(|record| record.runnability == SessionRunnability::Runnable)
                    .cloned();
                let grant = custody_grants.get(&execution_id).cloned();
                let reconcile = match (recovered.as_ref(), grant.as_ref()) {
                    (Some(record), Some(grant)) => (|| -> Result<()> {
                        let observed_material = observe_git_material(Path::new(&record.workspace));
                        let observed_material_identity =
                            material_observation_identity(&observed_material);
                        let checkpoint_generation =
                            upgrade_store.observe_checkpoint_generation(&state.request)?;
                        state.reconcile_successor_after_recovery(
                            executor_instance_id.clone(),
                            grant.generation.token.clone(),
                            Some(&implementation_revision),
                            observed_material_identity.as_deref(),
                            Some(checkpoint_generation),
                        )
                    })(),
                    _ => Err(anyhow!("D_SUCCESSOR_RECOVERY_OR_CUSTODY_FAILED")),
                };
                if let Err(error) = reconcile {
                    if let Some(grant) = custody_grants.remove(&execution_id) {
                        let _ = custody_provider.release(&grant);
                    }
                    if let Some(record) = records.get_mut(&execution_id) {
                        record.runnability = SessionRunnability::Quarantined;
                        record.custody_assessment = Some(not_held_custody());
                        let session_dir = session_root.join(&execution_id);
                        write_record(&session_dir, record)?;
                    }
                    state.mark_failed(error.to_string());
                }
                upgrade_store.save(state)?;
            }
        }
        Ok(Self {
            session_root,
            worktree_root,
            implementation_revision,
            device_id,
            executor_instance_id,
            recovery_summary,
            records: Mutex::new(records),
            managers: Mutex::new(managers),
            custody_provider: Mutex::new(custody_provider),
            custody_grants: Mutex::new(custody_grants),
            upgrade_store,
            upgrade_state: StdMutex::new(upgrade_state),
            admission_gate: RwLock::new(()),
        })
    }

    pub fn inspect_transactional_upgrade(&self) -> Option<TransactionalUpgradeState> {
        self.upgrade_state
            .lock()
            .expect("transactional upgrade mutex poisoned")
            .clone()
    }

    pub async fn prepare_transactional_upgrade(
        &self,
        execution_id: &str,
        attempt_id: &str,
        expected_successor_revision: &str,
        checkpoint_generation: u64,
    ) -> Result<TransactionalUpgradeState> {
        validate_sha(expected_successor_revision, "expected_successor_revision")?;
        if attempt_id.is_empty() {
            bail!("attempt_id must not be empty");
        }
        let _admission_barrier = self.admission_gate.write().await;
        let prior_upgrade = self
            .upgrade_state
            .lock()
            .expect("transactional upgrade mutex poisoned")
            .clone();
        if let Some(prior) = prior_upgrade.as_ref() {
            match prior.phase {
                UpgradePhase::Complete => {
                    if checkpoint_generation != prior.request.checkpoint_generation {
                        bail!("D_CHECKPOINT_PREPARE_MISMATCH");
                    }
                    self.upgrade_store.archive_completed(prior)?;
                }
                UpgradePhase::Failed => {
                    bail!("D_TRANSACTION_RECONCILIATION_REQUIRED");
                }
                _ => bail!("D_TRANSACTION_ALREADY_EXISTS"),
            }
        }
        let mut record = self.record(execution_id).await?;
        if record.lifecycle != SessionLifecycle::Ready
            || record.runnability != SessionRunnability::Runnable
        {
            bail!("D_EXECUTION_NOT_RUNNABLE");
        }
        let workspace = PathBuf::from(&record.workspace);
        let grant = self
            .custody_grants
            .lock()
            .await
            .get(execution_id)
            .cloned()
            .ok_or_else(|| anyhow!("D_PREDECESSOR_CUSTODY_NOT_CURRENT"))?;
        {
            let provider = self.custody_provider.lock().await;
            validate_effect_time_custody(
                &provider,
                &grant,
                execution_id,
                &workspace,
                &self.executor_instance_id,
            )?;
        }
        let scope = custody_scope(execution_id, &workspace)?;
        let request = TransactionalUpgradeRequest::new(
            new_transaction_id(),
            record.work_identity.clone(),
            attempt_id,
            execution_id,
            scope.collision_identity,
            self.executor_instance_id.clone(),
            grant.generation.token.clone(),
            self.implementation_revision.clone(),
            expected_successor_revision,
            material_identity(&record.expected_material),
            checkpoint_generation,
        )?;
        let mut state = TransactionalUpgradeState::new(request);
        state.quiesce()?;
        *self
            .upgrade_state
            .lock()
            .expect("transactional upgrade mutex poisoned") = Some(state.clone());

        if let Some(manager) = self.managers.lock().await.get(execution_id).cloned() {
            if let Err(error) = manager.close_all().await {
                state.mark_failed(format!("D_QUIESCE_FAILED: {error}"));
                self.upgrade_store.save(&state)?;
                *self
                    .upgrade_state
                    .lock()
                    .expect("transactional upgrade mutex poisoned") = Some(state);
                return Err(error).context("D_QUIESCE_FAILED");
            }
        }
        state.persist_transition()?;
        self.upgrade_store.save(&state)?;
        self.upgrade_store.persist_checkpoint(&state.request)?;
        *self
            .upgrade_state
            .lock()
            .expect("transactional upgrade mutex poisoned") = Some(state.clone());

        if let Err(error) = self.custody_provider.lock().await.release(&grant) {
            return Err(error).context("D_PREDECESSOR_RELEASE_FAILED");
        }
        self.custody_grants.lock().await.remove(execution_id);
        record.custody_assessment = Some(not_held_custody());
        durable_write_json(
            &self.session_root.join(execution_id).join("session.json"),
            &record,
        )?;
        self.records
            .lock()
            .await
            .insert(execution_id.to_owned(), record);
        state.release_predecessor_generation()?;
        self.upgrade_store.save(&state)?;
        *self
            .upgrade_state
            .lock()
            .expect("transactional upgrade mutex poisoned") = Some(state.clone());
        Ok(state)
    }

    pub fn resume_transactional_upgrade(
        &self,
        intent: ResumeIntent,
        external_event_payload: &str,
    ) -> Result<TransactionalUpgradeState> {
        let mut guard = self
            .upgrade_state
            .lock()
            .expect("transactional upgrade mutex poisoned");
        let state = guard
            .as_mut()
            .ok_or_else(|| anyhow!("D_TRANSACTION_NOT_FOUND"))?;
        state.accept_resume_durably(&self.upgrade_store, &intent, external_event_payload)?;
        Ok(state.clone())
    }

    fn ensure_transactional_admissions_open(&self) -> Result<()> {
        if let Some(state) = self
            .upgrade_state
            .lock()
            .expect("transactional upgrade mutex poisoned")
            .as_ref()
            && !state.admissions_open
        {
            bail!("D_ADMISSIONS_QUIESCED");
        }
        Ok(())
    }

    #[doc(hidden)]
    pub fn transactional_upgrade_fixture()
    -> crate::transactional_upgrade::TransactionalUpgradeHarness {
        crate::transactional_upgrade::TransactionalUpgradeHarness::fixture()
    }

    pub fn recovery_summary(&self) -> &RecoverySummary {
        &self.recovery_summary
    }

    pub fn executor_instance_id(&self) -> &str {
        &self.executor_instance_id
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
        let active_writers = self
            .custody_grants
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for record in self.records.lock().await.values() {
            if record.lifecycle != SessionLifecycle::Ready
                || !active_writers.contains(&record.execution_id)
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
        let _admission_guard = self.admission_gate.read().await;
        self.ensure_transactional_admissions_open()?;
        Uuid::parse_str(&request.execution_id).context("execution_id must be a UUID")?;
        if request.work_identity.is_empty() {
            bail!("work_identity must not be empty");
        }
        validate_sha(&request.base_sha, "base_sha")?;
        let write_requested =
            request.worktree_path.is_none() || request.access_mode == Some(AccessMode::Write);
        if write_requested {
            let digest = request
                .authorization_digest
                .as_deref()
                .ok_or_else(|| anyhow!("EFFECT_AUTHORIZATION_DIGEST_MISSING"))?;
            validate_authorization_digest(digest)?;
        }
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
        let effect_authorization = if binding.access_mode == AccessMode::Write {
            Some(EffectAuthorizationBinding {
                work_id: request.work_identity.clone(),
                execution_id: request.execution_id.clone(),
                resource_identity: request.repository.clone(),
                workspace_identity: workspace
                    .canonicalize()
                    .context("canonicalize effect workspace")?
                    .display()
                    .to_string(),
                candidate_revision: request.base_sha.clone(),
                effect_class: WRITE_PROCESS_EFFECT_CLASS.to_owned(),
                authorization_digest: request
                    .authorization_digest
                    .clone()
                    .ok_or_else(|| anyhow!("EFFECT_AUTHORIZATION_DIGEST_MISSING"))?,
            })
        } else {
            None
        };
        let custody_grant = if binding.access_mode == AccessMode::Write {
            let request = CustodyAcquireRequest {
                scope: custody_scope(&request.execution_id, &workspace)?,
                executor_instance_id: self.executor_instance_id.clone(),
            };
            match self.custody_provider.lock().await.acquire(request) {
                Ok(grant) => Some(grant),
                Err(error) => {
                    if managed_created {
                        let _ = remove_managed_worktree(&repository_path, &workspace);
                    }
                    let _ = std::fs::remove_dir_all(&session_dir);
                    if error.to_string().contains("CUSTODY_NOT_CURRENT") {
                        bail!("WORKTREE_BUSY: {}", workspace.display());
                    }
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
                if let Some(grant) = custody_grant.as_ref() {
                    let _ = self.custody_provider.lock().await.release(grant);
                }
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
            base_sha: request.base_sha.clone(),
            branch: session_branch,
            workspace: workspace.display().to_string(),
            worktree_binding: Some(binding),
            lifecycle: SessionLifecycle::Ready,
            runnability: SessionRunnability::Runnable,
            workspace_origin: if managed_created {
                WorkspaceOrigin::Managed
            } else {
                WorkspaceOrigin::External
            },
            expected_material: ExpectedMaterial {
                head_sha: request.base_sha.clone(),
                local_material: LocalMaterialState::None,
            },
            custody_assessment: match custody_grant.as_ref() {
                Some(grant) => Some(self.custody_provider.lock().await.assess(grant)),
                None => None,
            },
            effect_authorization,
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
        if let Some(grant) = custody_grant {
            self.custody_grants
                .lock()
                .await
                .insert(record.execution_id.clone(), grant);
        }
        self.inspect_session(&record.execution_id).await
    }

    pub async fn inspect_session(&self, execution_id: &str) -> Result<SessionInspection> {
        let record = self.record(execution_id).await?;
        let mut binding = record_binding(&record)?;
        let workspace = PathBuf::from(&record.workspace);
        let (head_sha, clean) = if workspace.is_dir() {
            if let Some(device_id) = &self.device_id {
                binding.device_id = Some(device_id.clone());
                if let Ok(common_dir) = git_common_dir(&workspace) {
                    binding.git_common_dir = Some(common_dir.display().to_string());
                }
            }
            let observed = observe_git_material(&workspace);
            (
                observed
                    .head_sha
                    .unwrap_or_else(|| record.expected_material.head_sha.clone()),
                observed.local_material == LocalMaterialState::None,
            )
        } else {
            (record.expected_material.head_sha.clone(), false)
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
            state: record.lifecycle,
            lifecycle: record.lifecycle,
            runnability: record.runnability,
            workspace_origin: record.workspace_origin,
            custody_assessment: record.custody_assessment,
            effect_authorization: record.effect_authorization,
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
        authorization_digest: Option<String>,
    ) -> Result<ExecutionProcessRef> {
        let _admission_guard = self.admission_gate.read().await;
        self.ensure_transactional_admissions_open()?;
        let manager = self.manager(execution_id).await?;
        let record = self.record(execution_id).await?;
        let binding = record_binding(&record)?;
        if binding.access_mode == AccessMode::Write {
            validate_material_authorization(
                &record,
                Path::new(&record.workspace),
                authorization_digest.as_deref(),
            )?;
            let grant = self
                .custody_grants
                .lock()
                .await
                .get(execution_id)
                .cloned()
                .ok_or_else(|| anyhow!("CUSTODY_NOT_CURRENT: execution has no current grant"))?;
            let provider = self.custody_provider.lock().await;
            validate_effect_time_custody(
                &provider,
                &grant,
                execution_id,
                Path::new(&record.workspace),
                &self.executor_instance_id,
            )?;
            let local = manager.start(argv, cwd).await?;
            return Ok(ExecutionProcessRef {
                process_id: namespace_process_id(execution_id, &local.process_id),
            });
        }
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
        let _admission_guard = self.admission_gate.read().await;
        self.ensure_transactional_admissions_open()?;
        let mut record = self.record(execution_id).await?;
        if record.lifecycle == SessionLifecycle::Closed {
            if custody_release_unresolved(&record) {
                bail!("CUSTODY_RELEASE_UNRESOLVED: closed session requires reconciliation");
            }
            return self.inspect_session(execution_id).await;
        }
        if let Some(manager) = self.managers.lock().await.get(execution_id).cloned() {
            manager.close_all().await?;
        }

        let current_grant = self.custody_grants.lock().await.get(execution_id).cloned();
        record.lifecycle = SessionLifecycle::Closed;
        record.runnability = SessionRunnability::Quarantined;
        record.closed_at = Some(now_millis()?);
        if current_grant.is_some() {
            record.custody_assessment = Some(CustodyAssessment {
                state: CustodyState::Unknown,
                assessed_at: now_millis()?,
                executor_instance_id: None,
                generation: None,
                evidence_ref: Some("CUSTODY_RELEASE_PENDING".to_owned()),
            });
        }

        // C requires CLOSED to reach durable storage before current B custody is released.
        durable_write_json(
            &self.session_root.join(execution_id).join("session.json"),
            &record,
        )?;
        self.records
            .lock()
            .await
            .insert(execution_id.to_owned(), record.clone());

        if let Some(grant) = current_grant {
            if let Err(error) = self.custody_provider.lock().await.release(&grant) {
                record.custody_assessment = Some(CustodyAssessment {
                    state: CustodyState::Unknown,
                    assessed_at: now_millis()?,
                    executor_instance_id: None,
                    generation: None,
                    evidence_ref: Some("CUSTODY_RELEASE_UNRESOLVED".to_owned()),
                });
                durable_write_json(
                    &self.session_root.join(execution_id).join("session.json"),
                    &record,
                )?;
                self.records
                    .lock()
                    .await
                    .insert(execution_id.to_owned(), record);
                return Err(error).context("CUSTODY_RELEASE_UNRESOLVED");
            }
            self.custody_grants.lock().await.remove(execution_id);
            record.custody_assessment = Some(not_held_custody());
            durable_write_json(
                &self.session_root.join(execution_id).join("session.json"),
                &record,
            )?;
            self.records
                .lock()
                .await
                .insert(execution_id.to_owned(), record);
        }
        self.managers.lock().await.remove(execution_id);
        self.inspect_session(execution_id).await
    }

    pub async fn retire_workspace(&self, intent: RetirementIntent) -> Result<RetirementReceipt> {
        let _admission_guard = self.admission_gate.read().await;
        self.ensure_transactional_admissions_open()?;
        Uuid::parse_str(&intent.execution_id).context("execution_id must be a UUID")?;
        validate_retirement_intent_id(&intent.retirement_intent_id)?;
        let session_dir = self.session_root.join(&intent.execution_id);
        let receipt_path = retirement_receipt_path(&session_dir, &intent.retirement_intent_id);
        if receipt_path.is_file() {
            let receipt: RetirementReceipt = serde_json::from_slice(
                &std::fs::read(&receipt_path).context("read retirement receipt")?,
            )?;
            if receipt.intent != intent {
                bail!("RETIREMENT_INTENT_REPLAY_MISMATCH");
            }
            return Ok(receipt);
        }

        let record = self.record(&intent.execution_id).await?;
        let intent_preexisted = persist_retirement_intent(&session_dir, &intent)?;

        if record.lifecycle != SessionLifecycle::Closed {
            return self
                .finish_retirement(
                    &session_dir,
                    intent,
                    RetirementOutcome::Denied,
                    Some(RetirementReasonCode::SessionNotClosed),
                    None,
                )
                .await;
        }
        let binding = record_binding(&record)?;
        if record.workspace_origin != WorkspaceOrigin::Managed
            || binding.mode != WorktreeMode::Managed
            || binding.ownership != WorktreeOwnership::Harness
        {
            return self
                .finish_retirement(
                    &session_dir,
                    intent,
                    RetirementOutcome::Denied,
                    Some(RetirementReasonCode::NotManagedHarness),
                    None,
                )
                .await;
        }
        if let Err(reason) = validate_retirement_authorization(&record, &intent) {
            return self
                .finish_retirement(
                    &session_dir,
                    intent,
                    RetirementOutcome::Denied,
                    Some(reason),
                    None,
                )
                .await;
        }

        let workspace = PathBuf::from(&record.workspace);
        if !workspace.exists() {
            let (outcome, reason) = if intent_preexisted {
                (
                    RetirementOutcome::OutcomeAmbiguous,
                    Some(RetirementReasonCode::OutcomeAmbiguous),
                )
            } else {
                (RetirementOutcome::AlreadyAbsentObserved, None)
            };
            return self
                .finish_retirement(&session_dir, intent, outcome, reason, None)
                .await;
        }

        let grant = match self
            .custody_provider
            .lock()
            .await
            .acquire(CustodyAcquireRequest {
                scope: custody_scope(&record.execution_id, &workspace)?,
                executor_instance_id: self.executor_instance_id.clone(),
            }) {
            Ok(grant) => grant,
            Err(_) => {
                return self
                    .finish_retirement(
                        &session_dir,
                        intent,
                        RetirementOutcome::Denied,
                        Some(RetirementReasonCode::CustodyNotCurrent),
                        None,
                    )
                    .await;
            }
        };
        let generation = Some(grant.generation.clone());

        let result = self
            .retire_with_current_grant(&record, &intent, &workspace, &grant)
            .await;
        let release_failed = self.custody_provider.lock().await.release(&grant).is_err();
        let (outcome, reason) = reconcile_retirement_release(result, release_failed);
        self.finish_retirement(&session_dir, intent, outcome, reason, generation)
            .await
    }

    async fn retire_with_current_grant(
        &self,
        record: &SessionRecord,
        intent: &RetirementIntent,
        workspace: &Path,
        grant: &CustodyGrant,
    ) -> (RetirementOutcome, Option<RetirementReasonCode>) {
        if validate_retirement_authorization(record, intent).is_err() {
            return (
                RetirementOutcome::Denied,
                Some(RetirementReasonCode::ScopeMismatch),
            );
        }
        let custody_current = {
            let provider = self.custody_provider.lock().await;
            validate_effect_time_custody(
                &provider,
                grant,
                &record.execution_id,
                workspace,
                &self.executor_instance_id,
            )
            .is_ok()
        };
        if !custody_current {
            return (
                RetirementOutcome::Denied,
                Some(RetirementReasonCode::CustodyNotCurrent),
            );
        }
        let preflight = observe_git_material(workspace);
        match classify_retirement_material(&preflight, &intent.expected_material) {
            Ok(()) => {}
            Err(reason) => return (RetirementOutcome::Denied, Some(reason)),
        }

        // Revalidate every positive predicate immediately before the destructive effect.
        if validate_retirement_authorization(record, intent).is_err() {
            return (
                RetirementOutcome::Denied,
                Some(RetirementReasonCode::ScopeMismatch),
            );
        }
        let custody_current = {
            let provider = self.custody_provider.lock().await;
            validate_effect_time_custody(
                &provider,
                grant,
                &record.execution_id,
                workspace,
                &self.executor_instance_id,
            )
            .is_ok()
        };
        if !custody_current {
            return (
                RetirementOutcome::Denied,
                Some(RetirementReasonCode::CustodyNotCurrent),
            );
        }
        let final_observation = observe_git_material(workspace);
        if final_observation.head_sha != preflight.head_sha
            || final_observation.local_material != preflight.local_material
        {
            return (
                RetirementOutcome::Denied,
                Some(RetirementReasonCode::MaterialChanged),
            );
        }
        match classify_retirement_material(&final_observation, &intent.expected_material) {
            Ok(()) => {}
            Err(reason) => return (RetirementOutcome::Denied, Some(reason)),
        }

        match remove_managed_worktree(Path::new(&record.repository_path), workspace) {
            Ok(()) => (RetirementOutcome::Retired, None),
            Err(_) if !workspace.exists() => (
                RetirementOutcome::OutcomeAmbiguous,
                Some(RetirementReasonCode::OutcomeAmbiguous),
            ),
            Err(_) => (
                RetirementOutcome::Failed,
                Some(RetirementReasonCode::EffectFailed),
            ),
        }
    }

    async fn finish_retirement(
        &self,
        session_dir: &Path,
        intent: RetirementIntent,
        outcome: RetirementOutcome,
        reason: Option<RetirementReasonCode>,
        custody_generation: Option<crate::session::CustodyGenerationEvidence>,
    ) -> Result<RetirementReceipt> {
        let receipt = RetirementReceipt {
            schema_version: RETIREMENT_RECEIPT_SCHEMA_VERSION.to_owned(),
            intent: intent.clone(),
            outcome,
            reason,
            custody_generation,
            observed_at: now_millis()?,
        };
        let target = retirement_receipt_path(session_dir, &intent.retirement_intent_id);
        if let Err(error) = durable_write_json(&target, &receipt) {
            if receipt.outcome == RetirementOutcome::Retired {
                return Ok(RetirementReceipt {
                    outcome: RetirementOutcome::OutcomeAmbiguous,
                    reason: Some(RetirementReasonCode::OutcomeAmbiguous),
                    ..receipt
                });
            }
            return Err(error).context("persist retirement receipt");
        }
        Ok(receipt)
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
        if record.lifecycle == SessionLifecycle::Closed {
            bail!("execution is closed: {execution_id}");
        }
        if record.lifecycle != SessionLifecycle::Ready {
            bail!("execution is not ready: {execution_id}");
        }
        if record.runnability != SessionRunnability::Runnable {
            bail!("execution is not runnable: {execution_id}");
        }
        self.managers
            .lock()
            .await
            .get(execution_id)
            .cloned()
            .ok_or_else(|| anyhow!("execution manager is unavailable: {execution_id}"))
    }
}

fn reconcile_retirement_release(
    result: (RetirementOutcome, Option<RetirementReasonCode>),
    release_failed: bool,
) -> (RetirementOutcome, Option<RetirementReasonCode>) {
    if release_failed {
        return (
            RetirementOutcome::OutcomeAmbiguous,
            Some(RetirementReasonCode::OutcomeAmbiguous),
        );
    }
    result
}

fn custody_release_unresolved(record: &SessionRecord) -> bool {
    record
        .custody_assessment
        .as_ref()
        .and_then(|assessment| assessment.evidence_ref.as_deref())
        .map(|value| {
            matches!(
                value,
                "CUSTODY_RELEASE_UNRESOLVED" | "CUSTODY_RELEASE_PENDING"
            )
        })
        .unwrap_or(false)
}

fn validate_retirement_intent_id(value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        bail!("RETIREMENT_INTENT_ID_INVALID");
    }
    Ok(())
}

fn retirement_intent_path(session_dir: &Path, intent_id: &str) -> PathBuf {
    session_dir
        .join("evidence/retirement/intents")
        .join(format!("{intent_id}.json"))
}

fn retirement_receipt_path(session_dir: &Path, intent_id: &str) -> PathBuf {
    session_dir
        .join("evidence/retirement/receipts")
        .join(format!("{intent_id}.json"))
}

fn persist_retirement_intent(session_dir: &Path, intent: &RetirementIntent) -> Result<bool> {
    let path = retirement_intent_path(session_dir, &intent.retirement_intent_id);
    if path.is_file() {
        let prior: RetirementIntent =
            serde_json::from_slice(&std::fs::read(&path).context("read retirement intent")?)?;
        if prior != *intent {
            bail!("RETIREMENT_INTENT_REPLAY_MISMATCH");
        }
        return Ok(true);
    }
    durable_write_json(&path, intent).context("persist retirement intent")?;
    Ok(false)
}

fn validate_retirement_authorization(
    record: &SessionRecord,
    intent: &RetirementIntent,
) -> std::result::Result<(), RetirementReasonCode> {
    let Some(binding) = intent.authorization_binding.as_ref() else {
        return Err(RetirementReasonCode::AuthorizationMissing);
    };
    if intent.effect_class != WORKSPACE_RETIRE_EFFECT_CLASS
        || intent.work_id != record.work_identity
        || intent.execution_id != record.execution_id
        || intent.candidate_revision != record.base_sha
        || intent.expected_material != record.expected_material
    {
        return Err(RetirementReasonCode::ScopeMismatch);
    }
    let observed_workspace = PathBuf::from(&record.workspace)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(&record.workspace))
        .display()
        .to_string();
    if intent.workspace_identity != observed_workspace {
        return Err(RetirementReasonCode::ScopeMismatch);
    }
    let claim = MaterialAuthorizationClaim {
        work_id: intent.work_id.clone(),
        execution_id: intent.execution_id.clone(),
        resource_identity: intent.resource_identity.clone(),
        workspace_identity: intent.workspace_identity.clone(),
        candidate_revision: intent.candidate_revision.clone(),
        effect_class: intent.effect_class.clone(),
        authorization_digest: intent.authorization_digest.clone(),
    };
    binding
        .validate(&claim)
        .map_err(|_| RetirementReasonCode::ScopeMismatch)
}

fn classify_retirement_material(
    observed: &crate::recovery::MaterialObservation,
    expected: &ExpectedMaterial,
) -> std::result::Result<(), RetirementReasonCode> {
    if observed.local_material == LocalMaterialState::Unknown || observed.head_sha.is_none() {
        return Err(RetirementReasonCode::MaterialUnknown);
    }
    if observed.local_material == LocalMaterialState::Present {
        return Err(RetirementReasonCode::MaterialPresent);
    }
    if observed.head_sha.as_deref() != Some(expected.head_sha.as_str())
        || observed.local_material != expected.local_material
    {
        return Err(RetirementReasonCode::MaterialChanged);
    }
    Ok(())
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

fn validate_material_authorization(
    record: &SessionRecord,
    workspace: &Path,
    presented_digest: Option<&str>,
) -> Result<()> {
    let expected = record
        .effect_authorization
        .as_ref()
        .ok_or_else(|| anyhow!("EFFECT_AUTHORIZATION_MISSING"))?;
    let digest = presented_digest.ok_or_else(|| anyhow!("EFFECT_AUTHORIZATION_DIGEST_MISSING"))?;
    let claim = MaterialAuthorizationClaim {
        work_id: record.work_identity.clone(),
        execution_id: record.execution_id.clone(),
        resource_identity: record.repository.clone(),
        workspace_identity: workspace
            .canonicalize()
            .context("canonicalize material authorization workspace")?
            .display()
            .to_string(),
        candidate_revision: record.base_sha.clone(),
        effect_class: WRITE_PROCESS_EFFECT_CLASS.to_owned(),
        authorization_digest: digest.to_owned(),
    };
    expected.validate(&claim)
}

fn validate_effect_time_custody(
    provider: &LocalCustodyProvider,
    grant: &CustodyGrant,
    execution_id: &str,
    workspace: &Path,
    executor_instance_id: &str,
) -> Result<()> {
    let claim = CustodyEffectClaim {
        execution_id: execution_id.to_owned(),
        collision_identity: custody_scope(execution_id, workspace)?.collision_identity,
        executor_instance_id: executor_instance_id.to_owned(),
        generation: Some(grant.generation.clone()),
    };
    provider.validate_effect(&claim)
}

fn custody_scope(execution_id: &str, workspace: &Path) -> Result<CustodyScope> {
    let collision_identity = workspace
        .canonicalize()
        .context("canonicalize custody workspace")?
        .display()
        .to_string();
    Ok(CustodyScope {
        execution_id: execution_id.to_owned(),
        collision_identity,
    })
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

#[cfg(test)]
fn indeterminate_custody_after_local_lock_probe() -> CustodyAssessment {
    CustodyAssessment {
        state: CustodyState::Unknown,
        assessed_at: now_millis().unwrap_or(0),
        executor_instance_id: None,
        generation: None,
        evidence_ref: None,
    }
}

fn not_held_custody() -> CustodyAssessment {
    CustodyAssessment {
        state: CustodyState::NotHeld,
        assessed_at: now_millis().unwrap_or(0),
        executor_instance_id: None,
        generation: None,
        evidence_ref: None,
    }
}

fn exact_binding_matches(binding: &WorktreeBinding, workspace: &Path) -> bool {
    let Ok(observed) = workspace.canonicalize() else {
        return false;
    };
    let Ok(expected) = PathBuf::from(&binding.path).canonicalize() else {
        return false;
    };
    observed == expected
}

fn pending_upgrade_targets(state: Option<&TransactionalUpgradeState>, execution_id: &str) -> bool {
    state
        .filter(|state| {
            state.phase != UpgradePhase::Complete && state.phase != UpgradePhase::Failed
        })
        .map(|state| state.request.execution_id == execution_id)
        .unwrap_or(false)
}

fn recovery_revision_matches(
    state: Option<&TransactionalUpgradeState>,
    record: &SessionRecord,
    observed_revision: &str,
) -> bool {
    let Some(state) = state.filter(|state| state.request.execution_id == record.execution_id)
    else {
        return record.implementation_revision == observed_revision;
    };
    match state.phase {
        UpgradePhase::Failed => false,
        UpgradePhase::Complete => record.implementation_revision == observed_revision,
        _ => {
            observed_revision == state.request.expected_successor_runtime_identity
                && (record.implementation_revision == state.request.predecessor_runtime_identity
                    || record.implementation_revision
                        == state.request.expected_successor_runtime_identity)
        }
    }
}

fn recovery_receipt(
    record: &SessionRecord,
    observed_executor_revision: &str,
    observed_workspace: Option<String>,
    observed_material: crate::recovery::MaterialObservation,
    custody: CustodyAssessment,
    result: SessionRunnability,
    reason_codes: Vec<RecoveryReasonCode>,
) -> SessionRecoveryReceipt {
    SessionRecoveryReceipt {
        schema_version: RECOVERY_RECEIPT_SCHEMA_VERSION.to_owned(),
        session_id: record.execution_id.clone(),
        expected_executor_revision: record.implementation_revision.clone(),
        observed_executor_revision: observed_executor_revision.to_owned(),
        expected_workspace: record.workspace.clone(),
        observed_workspace,
        expected_material: record.expected_material.clone(),
        observed_at: observed_material.observed_at,
        observed_material,
        custody,
        result,
        reason_codes,
    }
}

fn write_recovery_receipt(
    session_dir: &Path,
    executor_instance_id: &str,
    receipt: &SessionRecoveryReceipt,
) -> Result<()> {
    let root = session_dir.join("evidence/recovery");
    std::fs::create_dir_all(&root).context("create recovery receipt directory")?;
    let target = root.join(format!("{executor_instance_id}.json"));
    let temporary = root.join(format!("{executor_instance_id}.json.tmp"));
    std::fs::write(&temporary, serde_json::to_vec_pretty(receipt)?)
        .context("write recovery receipt")?;
    std::fs::rename(&temporary, &target).context("commit recovery receipt")?;
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

#[cfg(test)]
mod increment_a_contract_tests {
    use super::*;

    const B_AUTH_DIGEST: &str = "abababababababababababababababababababababababababababababababab";

    #[test]
    fn local_lock_probe_does_not_mint_custody_generation() {
        let assessment = indeterminate_custody_after_local_lock_probe();
        assert_eq!(assessment.state, CustodyState::Unknown);
        assert_eq!(assessment.executor_instance_id, None);
        assert_eq!(assessment.generation, None);
    }

    fn make_b_test_repo(path: &Path) -> String {
        std::fs::create_dir_all(path).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(path)
                .status()
                .unwrap()
                .success()
        );
        git(path, &["config", "user.name", "Custody B Test"]).unwrap();
        git(path, &["config", "user.email", "custody-b@example.invalid"]).unwrap();
        std::fs::write(path.join("README.md"), "b\n").unwrap();
        std::fs::write(path.join(".gitignore"), "effect-*\n").unwrap();
        git(path, &["add", "README.md", ".gitignore"]).unwrap();
        git(path, &["commit", "-q", "-m", "b"]).unwrap();
        git(path, &["rev-parse", "HEAD"]).unwrap()
    }

    #[test]
    fn effect_time_process_start_rejects_released_generation() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let id = "ffffffff-1111-4111-8111-111111111111";
        let executor = "executor-b-test";
        let mut provider =
            LocalCustodyProvider::new("TEST_LOCAL_FLOCK", root.path().join("locks")).unwrap();
        let grant = provider
            .acquire(CustodyAcquireRequest {
                scope: custody_scope(id, &workspace).unwrap(),
                executor_instance_id: executor.to_owned(),
            })
            .unwrap();
        provider.release(&grant).unwrap();
        let error =
            validate_effect_time_custody(&provider, &grant, id, &workspace, executor).unwrap_err();
        assert!(error.to_string().contains("CUSTODY_NOT_CURRENT"));
    }
    #[tokio::test]
    async fn stale_generation_is_rejected_at_actual_material_effect_boundary() {
        if !Path::new("/usr/bin/bwrap").is_file() {
            return; // Native-bwrap acceptance environment proves the material effects.
        }
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo-material-fence");
        let sha = make_b_test_repo(&repo);
        let sessions = root.path().join("sessions-material-fence");
        let id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        let revision = "edededededededededededededededededededed";

        let first = ProjectExecutorSupervisor::create(&sessions, revision).unwrap();
        let created = first
            .create_session(SessionCreateRequest {
                execution_id: id.to_owned(),
                work_identity: "work:test:b-material-fence".to_owned(),
                repository: repo.display().to_string(),
                base_sha: sha,
                worktree_path: None,
                access_mode: None,
                authorization_digest: Some(B_AUTH_DIGEST.to_owned()),
            })
            .await
            .unwrap();
        let workspace = PathBuf::from(created.workspace.clone());
        let g1 = first.custody_grants.lock().await.get(id).cloned().unwrap();
        let a = first
            .start_process(
                id,
                vec![
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    "printf A > effect-A".to_owned(),
                ],
                None,
                Some(B_AUTH_DIGEST.to_owned()),
            )
            .await
            .unwrap();
        first.wait_process(id, &a.process_id, None).await.unwrap();
        assert!(workspace.join("effect-A").is_file());
        drop(first);

        let second = ProjectExecutorSupervisor::create(&sessions, revision).unwrap();
        let g2 = second.custody_grants.lock().await.get(id).cloned().unwrap();
        assert_ne!(g1.generation, g2.generation);

        let wrong_authorization = second
            .start_process(
                id,
                vec![
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    "printf X > effect-X".to_owned(),
                ],
                None,
                Some("cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd".to_owned()),
            )
            .await
            .unwrap_err();
        assert!(
            wrong_authorization
                .to_string()
                .contains("EFFECT_AUTHORIZATION_DIGEST_MISMATCH")
        );
        assert!(!workspace.join("effect-X").exists());

        second
            .custody_grants
            .lock()
            .await
            .insert(id.to_owned(), g1.clone());
        let stale = second
            .start_process(
                id,
                vec![
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    "printf B > effect-B".to_owned(),
                ],
                None,
                Some(B_AUTH_DIGEST.to_owned()),
            )
            .await
            .unwrap_err();
        assert!(stale.to_string().contains("CUSTODY_GENERATION_STALE"));
        assert!(!workspace.join("effect-B").exists());

        second.custody_grants.lock().await.insert(id.to_owned(), g2);
        let c = second
            .start_process(
                id,
                vec![
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    "printf C > effect-C".to_owned(),
                ],
                None,
                Some(B_AUTH_DIGEST.to_owned()),
            )
            .await
            .unwrap();
        second.wait_process(id, &c.process_id, None).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.join("effect-A")).unwrap(),
            "A"
        );
        assert!(!workspace.join("effect-B").exists());
        assert_eq!(
            std::fs::read_to_string(workspace.join("effect-C")).unwrap(),
            "C"
        );
    }

    #[test]
    fn retirement_release_failure_is_never_terminal_success_or_denial() {
        let (outcome, reason) =
            reconcile_retirement_release((RetirementOutcome::Retired, None), true);
        assert_eq!(outcome, RetirementOutcome::OutcomeAmbiguous);
        assert_eq!(reason, Some(RetirementReasonCode::OutcomeAmbiguous));
    }

    #[tokio::test]
    async fn read_only_session_never_mints_write_custody() {
        if !Path::new("/usr/bin/bwrap").is_file() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo-read-only");
        let sha = make_b_test_repo(&repo);
        let external = root.path().join("external-read-only");
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["worktree", "add", "--detach"])
                .arg(&external)
                .arg(&sha)
                .status()
                .unwrap()
                .success()
        );
        let id = "bbbbbbbb-cccc-4ddd-8eee-ffffffffffff";
        let supervisor = ProjectExecutorSupervisor::create_with_worktree_root(
            root.path().join("sessions-read-only"),
            root.path(),
            "fdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfdfd",
        )
        .unwrap();
        let inspection = supervisor
            .create_session(SessionCreateRequest {
                execution_id: id.to_owned(),
                work_identity: "work:test:b-read-only".to_owned(),
                repository: repo.display().to_string(),
                base_sha: sha,
                worktree_path: Some(external.display().to_string()),
                access_mode: Some(AccessMode::Read),
                authorization_digest: None,
            })
            .await
            .unwrap();
        assert_eq!(inspection.runnability, SessionRunnability::Runnable);
        assert_eq!(inspection.custody_assessment, None);
        assert!(!supervisor.custody_grants.lock().await.contains_key(id));
    }
}
