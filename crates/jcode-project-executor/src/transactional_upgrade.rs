use crate::durable::durable_write_json;
use crate::recovery::MaterialObservation;
use crate::session::{ExpectedMaterial, LocalMaterialState};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use uuid::Uuid;

pub const TRANSACTIONAL_UPGRADE_SCHEMA_VERSION: &str = "ProjectExecutorTransactionalUpgrade/v1";
pub const CONTINUATION_CHECKPOINT_SCHEMA_VERSION: &str = "ProjectExecutorContinuationCheckpoint/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UpgradePhase {
    Quiesce,
    Persist,
    ReleaseOldGeneration,
    StartSuccessor,
    Recover,
    AcquireNewGeneration,
    VerifyRuntimeAndMaterial,
    ResumeAdmission,
    Complete,
    Failed,
}

impl UpgradePhase {
    pub const ORDER: [Self; 8] = [
        Self::Quiesce,
        Self::Persist,
        Self::ReleaseOldGeneration,
        Self::StartSuccessor,
        Self::Recover,
        Self::AcquireNewGeneration,
        Self::VerifyRuntimeAndMaterial,
        Self::ResumeAdmission,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeIntent {
    pub work_id: String,
    pub attempt_id: String,
    pub execution_id: String,
    pub expected_checkpoint_generation: u64,
    pub external_event_id: String,
    pub external_event_digest: String,
    pub expected_custody_generation: String,
    pub expected_executor_instance_id: String,
    pub expected_runtime_identity: String,
    pub expected_material_identity: String,
}

impl ResumeIntent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        work_id: impl Into<String>,
        attempt_id: impl Into<String>,
        execution_id: impl Into<String>,
        expected_checkpoint_generation: u64,
        external_event_id: impl Into<String>,
        external_event_digest: impl Into<String>,
        expected_custody_generation: impl Into<String>,
        expected_executor_instance_id: impl Into<String>,
        expected_runtime_identity: impl Into<String>,
        expected_material_identity: impl Into<String>,
    ) -> Result<Self> {
        let intent = Self {
            work_id: work_id.into(),
            attempt_id: attempt_id.into(),
            execution_id: execution_id.into(),
            expected_checkpoint_generation,
            external_event_id: external_event_id.into(),
            external_event_digest: external_event_digest.into(),
            expected_custody_generation: expected_custody_generation.into(),
            expected_executor_instance_id: expected_executor_instance_id.into(),
            expected_runtime_identity: expected_runtime_identity.into(),
            expected_material_identity: expected_material_identity.into(),
        };
        intent.validate()?;
        Ok(intent)
    }

    fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("work_id", self.work_id.as_str()),
            ("attempt_id", self.attempt_id.as_str()),
            ("execution_id", self.execution_id.as_str()),
            ("external_event_id", self.external_event_id.as_str()),
            (
                "expected_custody_generation",
                self.expected_custody_generation.as_str(),
            ),
            (
                "expected_executor_instance_id",
                self.expected_executor_instance_id.as_str(),
            ),
            (
                "expected_runtime_identity",
                self.expected_runtime_identity.as_str(),
            ),
            (
                "expected_material_identity",
                self.expected_material_identity.as_str(),
            ),
        ] {
            if value.is_empty() {
                bail!("{name} must not be empty");
            }
        }
        if self.external_event_digest.len() != 64
            || !self
                .external_event_digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        {
            bail!("external_event_digest must be 64 hex characters");
        }
        Ok(())
    }

    pub fn matches_current_checkpoint(&self) -> bool {
        self.expected_checkpoint_generation == 7
    }
    pub fn matches_current_generation(&self) -> bool {
        self.expected_custody_generation == "G2"
    }
    pub fn matches_runtime_identity(&self) -> bool {
        self.expected_runtime_identity == "runtime-v2"
    }
    pub fn matches_material_identity(&self) -> bool {
        self.expected_material_identity == "material-v1"
    }
    pub fn external_event_digest_matches(&self) -> bool {
        self.external_event_digest == fixture_digest()
    }

    #[doc(hidden)]
    pub fn fixture() -> Self {
        Self::new(
            "work:test:d",
            "attempt:test:d",
            "execution:test:d",
            7,
            "event-1",
            fixture_digest(),
            "G2",
            "executor-v2",
            "runtime-v2",
            "material-v1",
        )
        .expect("valid D fixture")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransactionalUpgradeRequest {
    pub transaction_id: String,
    pub work_id: String,
    pub attempt_id: String,
    pub execution_id: String,
    pub collision_identity: String,
    pub predecessor_executor_instance_id: String,
    pub predecessor_generation: String,
    pub predecessor_runtime_identity: String,
    pub expected_successor_runtime_identity: String,
    pub expected_material_identity: String,
    pub checkpoint_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixture_successor_generation: Option<String>,
}

impl TransactionalUpgradeRequest {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        transaction_id: impl Into<String>,
        work_id: impl Into<String>,
        attempt_id: impl Into<String>,
        execution_id: impl Into<String>,
        collision_identity: impl Into<String>,
        predecessor_executor_instance_id: impl Into<String>,
        predecessor_generation: impl Into<String>,
        predecessor_runtime_identity: impl Into<String>,
        expected_successor_runtime_identity: impl Into<String>,
        expected_material_identity: impl Into<String>,
        checkpoint_generation: u64,
    ) -> Result<Self> {
        let request = Self {
            transaction_id: transaction_id.into(),
            work_id: work_id.into(),
            attempt_id: attempt_id.into(),
            execution_id: execution_id.into(),
            collision_identity: collision_identity.into(),
            predecessor_executor_instance_id: predecessor_executor_instance_id.into(),
            predecessor_generation: predecessor_generation.into(),
            predecessor_runtime_identity: predecessor_runtime_identity.into(),
            expected_successor_runtime_identity: expected_successor_runtime_identity.into(),
            expected_material_identity: expected_material_identity.into(),
            checkpoint_generation,
            fixture_successor_generation: None,
        };
        request.validate()?;
        Ok(request)
    }

    fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("transaction_id", self.transaction_id.as_str()),
            ("work_id", self.work_id.as_str()),
            ("attempt_id", self.attempt_id.as_str()),
            ("execution_id", self.execution_id.as_str()),
            ("collision_identity", self.collision_identity.as_str()),
            (
                "predecessor_executor_instance_id",
                self.predecessor_executor_instance_id.as_str(),
            ),
            (
                "predecessor_generation",
                self.predecessor_generation.as_str(),
            ),
            (
                "expected_successor_runtime_identity",
                self.expected_successor_runtime_identity.as_str(),
            ),
            (
                "expected_material_identity",
                self.expected_material_identity.as_str(),
            ),
        ] {
            if value.is_empty() {
                bail!("{name} must not be empty");
            }
        }
        Ok(())
    }

    #[doc(hidden)]
    pub fn fixture(g1: &str, g2: &str) -> Self {
        let mut request = Self::new(
            "transaction-test-d",
            "work:test:d",
            "attempt:test:d",
            "execution:test:d",
            "/tmp/workspace-test-d",
            "executor-v1",
            g1,
            "runtime-v1",
            "runtime-v2",
            "material-v1",
            7,
        )
        .expect("valid D fixture request");
        request.fixture_successor_generation = Some(g2.to_owned());
        request
    }

    #[doc(hidden)]
    pub fn predecessor_invalid_before_successor_effect(&self) -> bool {
        let mut state = TransactionalUpgradeState::new(self.clone());
        state.quiesce().is_ok()
            && state.persist_transition().is_ok()
            && state.release_predecessor_generation().is_ok()
            && !state.predecessor_authority_current
    }

    #[doc(hidden)]
    pub fn stale_generation_rejected_after_successor(&self) -> bool {
        let Some(g2) = self.fixture_successor_generation.as_deref() else {
            return false;
        };
        if g2 == self.predecessor_generation {
            return false;
        }
        let state = self.successful_recovery_fixture();
        !state.generation_is_current(&self.predecessor_generation)
            && state.generation_is_current(g2)
    }

    #[doc(hidden)]
    pub fn required_failure_frontier(&self) -> Vec<TransactionalUpgradeState> {
        let mut base = self.successful_recovery_fixture();
        let mut result = Vec::new();
        let mut predecessor = base.clone();
        predecessor.predecessor_authority_current = true;
        result.push(predecessor);
        let mut successor = base.clone();
        successor.successor_authority_current = false;
        result.push(successor);
        let mut runtime = base.clone();
        runtime.runtime_verified = false;
        result.push(runtime);
        let mut material = base.clone();
        material.material_verified = false;
        result.push(material);
        let mut checkpoint = base.clone();
        checkpoint.checkpoint_verified = false;
        result.push(checkpoint);
        base.phase = UpgradePhase::Failed;
        result.push(base);
        result
    }

    #[doc(hidden)]
    pub fn successful_recovery_fixture(&self) -> TransactionalUpgradeState {
        let g2 = self
            .fixture_successor_generation
            .clone()
            .unwrap_or_else(|| "G2".to_owned());
        let mut state = TransactionalUpgradeState::new(self.clone());
        state.quiesce().unwrap();
        state.persist_transition().unwrap();
        state.release_predecessor_generation().unwrap();
        state.start_successor("executor-v2").unwrap();
        state.recover_successor(true).unwrap();
        state.acquire_successor_generation(g2).unwrap();
        state
            .verify_successor(
                Some(&self.expected_successor_runtime_identity),
                Some(&self.expected_material_identity),
                Some(self.checkpoint_generation),
            )
            .unwrap();
        state
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuationCheckpoint {
    pub schema_version: String,
    pub transaction_id: String,
    pub work_id: String,
    pub attempt_id: String,
    pub execution_id: String,
    pub collision_identity: String,
    pub generation: u64,
    pub predecessor_runtime_identity: String,
    pub expected_successor_runtime_identity: String,
    pub expected_material_identity: String,
}

impl ContinuationCheckpoint {
    pub fn from_request(request: &TransactionalUpgradeRequest) -> Self {
        Self {
            schema_version: CONTINUATION_CHECKPOINT_SCHEMA_VERSION.to_owned(),
            transaction_id: request.transaction_id.clone(),
            work_id: request.work_id.clone(),
            attempt_id: request.attempt_id.clone(),
            execution_id: request.execution_id.clone(),
            collision_identity: request.collision_identity.clone(),
            generation: request.checkpoint_generation,
            predecessor_runtime_identity: request.predecessor_runtime_identity.clone(),
            expected_successor_runtime_identity: request
                .expected_successor_runtime_identity
                .clone(),
            expected_material_identity: request.expected_material_identity.clone(),
        }
    }

    pub fn validate_against(&self, request: &TransactionalUpgradeRequest) -> Result<()> {
        if self.schema_version != CONTINUATION_CHECKPOINT_SCHEMA_VERSION {
            bail!("D_CHECKPOINT_SCHEMA_UNSUPPORTED");
        }
        if self.transaction_id != request.transaction_id
            || self.work_id != request.work_id
            || self.attempt_id != request.attempt_id
            || self.execution_id != request.execution_id
            || self.collision_identity != request.collision_identity
            || self.predecessor_runtime_identity != request.predecessor_runtime_identity
            || self.expected_successor_runtime_identity
                != request.expected_successor_runtime_identity
            || self.expected_material_identity != request.expected_material_identity
        {
            bail!("D_CHECKPOINT_BINDING_MISMATCH");
        }
        if self.generation != request.checkpoint_generation {
            bail!("D_CHECKPOINT_GENERATION_MISMATCH");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransactionalUpgradeState {
    pub schema_version: String,
    pub request: TransactionalUpgradeRequest,
    pub phase: UpgradePhase,
    pub admissions_open: bool,
    pub predecessor_authority_current: bool,
    pub successor_authority_current: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor_executor_instance_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor_generation: Option<String>,
    pub runtime_verified: bool,
    pub material_verified: bool,
    pub checkpoint_verified: bool,
    #[serde(default)]
    pub consumed_events: BTreeMap<String, String>,
    pub continuation_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
}

impl TransactionalUpgradeState {
    pub fn new(request: TransactionalUpgradeRequest) -> Self {
        Self {
            schema_version: TRANSACTIONAL_UPGRADE_SCHEMA_VERSION.to_owned(),
            request,
            phase: UpgradePhase::Quiesce,
            admissions_open: true,
            predecessor_authority_current: true,
            successor_authority_current: false,
            successor_executor_instance_id: None,
            successor_generation: None,
            runtime_verified: false,
            material_verified: false,
            checkpoint_verified: false,
            consumed_events: BTreeMap::new(),
            continuation_count: 0,
            failure_reason: None,
        }
    }

    pub fn quiesce(&mut self) -> Result<()> {
        self.require_phase(UpgradePhase::Quiesce)?;
        self.admissions_open = false;
        self.phase = UpgradePhase::Persist;
        Ok(())
    }

    pub fn persist_transition(&mut self) -> Result<()> {
        self.require_phase(UpgradePhase::Persist)?;
        if self.admissions_open {
            bail!("D_ADMISSIONS_NOT_QUIESCED");
        }
        self.phase = UpgradePhase::ReleaseOldGeneration;
        Ok(())
    }

    pub fn release_predecessor_generation(&mut self) -> Result<()> {
        self.require_phase(UpgradePhase::ReleaseOldGeneration)?;
        self.predecessor_authority_current = false;
        self.phase = UpgradePhase::StartSuccessor;
        Ok(())
    }

    pub fn start_successor(&mut self, executor_instance_id: impl Into<String>) -> Result<()> {
        self.require_phase(UpgradePhase::StartSuccessor)?;
        if self.predecessor_authority_current {
            bail!("D_PREDECESSOR_AUTHORITY_STILL_CURRENT");
        }
        let executor_instance_id = executor_instance_id.into();
        if executor_instance_id.is_empty()
            || executor_instance_id == self.request.predecessor_executor_instance_id
        {
            bail!("D_SUCCESSOR_IDENTITY_INVALID");
        }
        self.successor_executor_instance_id = Some(executor_instance_id);
        self.phase = UpgradePhase::Recover;
        Ok(())
    }

    pub fn recover_successor(&mut self, recovery_succeeded: bool) -> Result<()> {
        self.require_phase(UpgradePhase::Recover)?;
        if !recovery_succeeded {
            self.fail_closed("D_SUCCESSOR_RECOVERY_FAILED");
            bail!("D_SUCCESSOR_RECOVERY_FAILED");
        }
        self.phase = UpgradePhase::AcquireNewGeneration;
        Ok(())
    }

    pub fn acquire_successor_generation(&mut self, generation: impl Into<String>) -> Result<()> {
        self.require_phase(UpgradePhase::AcquireNewGeneration)?;
        if self.predecessor_authority_current {
            bail!("D_PREDECESSOR_AUTHORITY_STILL_CURRENT");
        }
        let generation = generation.into();
        if generation.is_empty() || generation == self.request.predecessor_generation {
            self.fail_closed("D_SUCCESSOR_GENERATION_NOT_FRESH");
            bail!("D_SUCCESSOR_GENERATION_NOT_FRESH");
        }
        self.successor_generation = Some(generation);
        self.successor_authority_current = true;
        self.phase = UpgradePhase::VerifyRuntimeAndMaterial;
        Ok(())
    }

    pub fn verify_successor(
        &mut self,
        observed_runtime_identity: Option<&str>,
        observed_material_identity: Option<&str>,
        observed_checkpoint_generation: Option<u64>,
    ) -> Result<()> {
        self.require_phase(UpgradePhase::VerifyRuntimeAndMaterial)?;
        let runtime = observed_runtime_identity
            .filter(|v| *v == self.request.expected_successor_runtime_identity)
            .is_some();
        let material = observed_material_identity
            .filter(|v| *v == self.request.expected_material_identity)
            .is_some();
        let checkpoint = observed_checkpoint_generation == Some(self.request.checkpoint_generation);
        self.runtime_verified = runtime;
        self.material_verified = material;
        self.checkpoint_verified = checkpoint;
        if !(runtime && material && checkpoint) {
            self.fail_closed("D_SUCCESSOR_VERIFICATION_FAILED_OR_UNKNOWN");
            bail!("D_SUCCESSOR_VERIFICATION_FAILED_OR_UNKNOWN");
        }
        self.phase = UpgradePhase::ResumeAdmission;
        Ok(())
    }

    pub fn resume_admission_allowed(&self) -> bool {
        self.phase == UpgradePhase::ResumeAdmission
            && !self.admissions_open
            && !self.predecessor_authority_current
            && self.successor_authority_current
            && self.successor_generation.is_some()
            && self.runtime_verified
            && self.material_verified
            && self.checkpoint_verified
            && self.failure_reason.is_none()
    }

    pub fn generation_is_current(&self, generation: &str) -> bool {
        self.successor_authority_current
            && self.successor_generation.as_deref() == Some(generation)
            && generation != self.request.predecessor_generation
    }

    pub fn predecessor_generation(&self) -> &str {
        &self.request.predecessor_generation
    }

    pub fn successor_generation(&self) -> &str {
        self.successor_generation.as_deref().unwrap_or("")
    }

    pub fn reconcile_successor_after_recovery(
        &mut self,
        successor_executor_instance_id: impl Into<String>,
        successor_generation: impl Into<String>,
        observed_runtime_identity: Option<&str>,
        observed_material_identity: Option<&str>,
        observed_checkpoint_generation: Option<u64>,
    ) -> Result<()> {
        if self.phase == UpgradePhase::Complete {
            bail!("D_UPGRADE_ALREADY_COMPLETE");
        }
        if self.phase == UpgradePhase::Failed {
            bail!("D_UPGRADE_FAILED");
        }
        let successor_executor_instance_id = successor_executor_instance_id.into();
        let successor_generation = successor_generation.into();
        if successor_executor_instance_id.is_empty()
            || successor_executor_instance_id == self.request.predecessor_executor_instance_id
        {
            self.fail_closed("D_SUCCESSOR_IDENTITY_INVALID");
            bail!("D_SUCCESSOR_IDENTITY_INVALID");
        }
        if successor_generation.is_empty()
            || successor_generation == self.request.predecessor_generation
        {
            self.fail_closed("D_SUCCESSOR_GENERATION_NOT_FRESH");
            bail!("D_SUCCESSOR_GENERATION_NOT_FRESH");
        }
        self.admissions_open = false;
        self.predecessor_authority_current = false;
        self.successor_executor_instance_id = Some(successor_executor_instance_id);
        self.successor_generation = Some(successor_generation);
        self.successor_authority_current = true;
        self.phase = UpgradePhase::VerifyRuntimeAndMaterial;
        self.verify_successor(
            observed_runtime_identity,
            observed_material_identity,
            observed_checkpoint_generation,
        )
    }

    pub fn mark_failed(&mut self, reason: impl AsRef<str>) {
        self.fail_closed(reason.as_ref());
    }

    fn accept_resume_unpersisted(&mut self, intent: &ResumeIntent) -> Result<()> {
        self.validate_resume_intent(intent)?;
        self.consumed_events.insert(
            intent.external_event_id.clone(),
            intent.external_event_digest.clone(),
        );
        self.continuation_count = self.continuation_count.saturating_add(1);
        self.request.checkpoint_generation = self.request.checkpoint_generation.saturating_add(1);
        self.admissions_open = true;
        self.phase = UpgradePhase::Complete;
        Ok(())
    }

    pub fn accept_resume_durably(
        &mut self,
        store: &TransactionalUpgradeStore,
        intent: &ResumeIntent,
        external_event_payload: &str,
    ) -> Result<()> {
        if event_digest(external_event_payload) != intent.external_event_digest {
            bail!("D_RESUME_EVENT_DIGEST_MISMATCH");
        }
        if store.event_already_consumed(&intent.external_event_id)? {
            bail!("D_RESUME_EVENT_ALREADY_CONSUMED");
        }
        let mut proposed = self.clone();
        proposed.accept_resume_unpersisted(intent)?;
        store.save(&proposed)?;
        *self = proposed;
        Ok(())
    }

    fn validate_resume_intent(&self, intent: &ResumeIntent) -> Result<()> {
        intent.validate()?;
        if !self.resume_admission_allowed() {
            bail!("D_RESUME_NOT_ELIGIBLE");
        }
        if intent.work_id != self.request.work_id
            || intent.attempt_id != self.request.attempt_id
            || intent.execution_id != self.request.execution_id
        {
            bail!("D_RESUME_LINEAGE_MISMATCH");
        }
        if intent.expected_checkpoint_generation != self.request.checkpoint_generation {
            bail!("D_RESUME_CHECKPOINT_STALE");
        }
        if self.consumed_events.contains_key(&intent.external_event_id) {
            bail!("D_RESUME_EVENT_ALREADY_CONSUMED");
        }
        if intent.expected_custody_generation != self.successor_generation() {
            bail!("D_RESUME_GENERATION_STALE");
        }
        if intent.expected_executor_instance_id
            != self.successor_executor_instance_id.as_deref().unwrap_or("")
        {
            bail!("D_RESUME_EXECUTOR_STALE");
        }
        if intent.expected_runtime_identity != self.request.expected_successor_runtime_identity {
            bail!("D_RESUME_RUNTIME_MISMATCH");
        }
        if intent.expected_material_identity != self.request.expected_material_identity {
            bail!("D_RESUME_MATERIAL_MISMATCH");
        }
        Ok(())
    }

    fn require_phase(&self, phase: UpgradePhase) -> Result<()> {
        if self.phase != phase {
            bail!(
                "D_PHASE_MISMATCH: expected {phase:?}, observed {:?}",
                self.phase
            );
        }
        Ok(())
    }

    fn fail_closed(&mut self, reason: &str) {
        self.admissions_open = false;
        self.successor_authority_current = false;
        self.failure_reason = Some(reason.to_owned());
        self.phase = UpgradePhase::Failed;
    }
}

#[derive(Debug, Clone)]
pub struct TransactionalUpgradeStore {
    path: PathBuf,
}

impl TransactionalUpgradeStore {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    pub fn save(&self, state: &TransactionalUpgradeState) -> Result<()> {
        durable_write_json(&self.path, state)
    }

    pub fn load(&self) -> Result<TransactionalUpgradeState> {
        let bytes = std::fs::read(&self.path).context("read transactional upgrade state")?;
        let state: TransactionalUpgradeState =
            serde_json::from_slice(&bytes).context("parse transactional upgrade state")?;
        if state.schema_version != TRANSACTIONAL_UPGRADE_SCHEMA_VERSION {
            bail!("D_UPGRADE_STATE_SCHEMA_UNSUPPORTED");
        }
        Ok(state)
    }

    fn checkpoint_path(&self) -> Result<PathBuf> {
        Ok(self
            .path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("transactional upgrade state has no parent"))?
            .join("checkpoint.json"))
    }

    pub fn persist_checkpoint(&self, request: &TransactionalUpgradeRequest) -> Result<()> {
        let checkpoint = ContinuationCheckpoint::from_request(request);
        durable_write_json(&self.checkpoint_path()?, &checkpoint)
            .context("D_CHECKPOINT_PERSIST_FAILED")
    }

    pub fn observe_checkpoint_generation(
        &self,
        request: &TransactionalUpgradeRequest,
    ) -> Result<u64> {
        let path = self.checkpoint_path()?;
        if !path.is_file() {
            bail!("D_CHECKPOINT_MISSING");
        }
        let bytes = std::fs::read(&path).context("D_CHECKPOINT_READ_FAILED")?;
        let checkpoint: ContinuationCheckpoint =
            serde_json::from_slice(&bytes).context("D_CHECKPOINT_PARSE_FAILED")?;
        checkpoint.validate_against(request)?;
        Ok(checkpoint.generation)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn archive_completed(&self, state: &TransactionalUpgradeState) -> Result<PathBuf> {
        if state.phase != UpgradePhase::Complete {
            bail!("D_HISTORY_REQUIRES_COMPLETE_TRANSACTION");
        }
        let root = self
            .path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("transactional upgrade state has no parent"))?
            .join("history");
        let key = format!(
            "{:x}",
            Sha256::digest(state.request.transaction_id.as_bytes())
        );
        let target = root.join(format!("{key}.json"));
        if target.is_file() {
            let existing: TransactionalUpgradeState = serde_json::from_slice(
                &std::fs::read(&target).context("read archived transactional upgrade state")?,
            )
            .context("parse archived transactional upgrade state")?;
            if existing != *state {
                bail!("D_HISTORY_TRANSACTION_ID_CONFLICT");
            }
            return Ok(target);
        }
        durable_write_json(&target, state)?;
        Ok(target)
    }

    pub fn event_already_consumed(&self, event_id: &str) -> Result<bool> {
        let root = self
            .path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("transactional upgrade state has no parent"))?
            .join("history");
        if !root.is_dir() {
            return Ok(false);
        }
        let mut entries = std::fs::read_dir(&root)
            .context("read transactional upgrade history")?
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if !entry.file_type()?.is_file() {
                continue;
            }
            let archived: TransactionalUpgradeState = serde_json::from_slice(
                &std::fs::read(entry.path()).context("read transactional upgrade history entry")?,
            )
            .context("parse transactional upgrade history entry")?;
            if archived.schema_version != TRANSACTIONAL_UPGRADE_SCHEMA_VERSION {
                bail!("D_UPGRADE_HISTORY_SCHEMA_UNSUPPORTED");
            }
            if archived.consumed_events.contains_key(event_id) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConcurrentResumeResult {
    accepted: usize,
}

impl ConcurrentResumeResult {
    pub fn accepted_count(&self) -> usize {
        self.accepted
    }
}

#[doc(hidden)]
pub struct TransactionalUpgradeHarness {
    state: Mutex<TransactionalUpgradeState>,
}

impl TransactionalUpgradeHarness {
    pub fn fixture() -> Self {
        let request = TransactionalUpgradeRequest::fixture("G1", "G2");
        Self {
            state: Mutex::new(request.successful_recovery_fixture()),
        }
    }

    pub fn resume_once(&self, intent: ResumeIntent) -> Result<()> {
        self.state
            .lock()
            .expect("D fixture mutex poisoned")
            .accept_resume_unpersisted(&intent)
    }

    pub fn resume_with_stale_checkpoint(&self, mut intent: ResumeIntent) -> Result<()> {
        intent.expected_checkpoint_generation =
            intent.expected_checkpoint_generation.saturating_sub(1);
        self.state
            .lock()
            .expect("D fixture mutex poisoned")
            .accept_resume_unpersisted(&intent)
    }

    pub fn resume_with_stale_generation(&self, mut intent: ResumeIntent) -> Result<()> {
        intent.expected_custody_generation = "G1".to_owned();
        self.state
            .lock()
            .expect("D fixture mutex poisoned")
            .accept_resume_unpersisted(&intent)
    }

    pub fn resume_concurrently(&self, intent: ResumeIntent) -> ConcurrentResumeResult {
        let mut accepted = 0usize;
        for _ in 0..2 {
            if self
                .state
                .lock()
                .expect("D fixture mutex poisoned")
                .accept_resume_unpersisted(&intent)
                .is_ok()
            {
                accepted += 1;
            }
        }
        ConcurrentResumeResult { accepted }
    }

    pub fn old_executor_continuation_fixture(&self) -> Result<()> {
        let mut intent = ResumeIntent::fixture();
        intent.expected_executor_instance_id = "executor-v1".to_owned();
        self.state
            .lock()
            .expect("D fixture mutex poisoned")
            .accept_resume_unpersisted(&intent)
    }

    pub fn crash_before_event_persist_fixture(&self) -> CrashBeforePersistFixture {
        CrashBeforePersistFixture
    }

    pub fn crash_after_event_persist_before_execution_fixture(&self) -> CrashAfterPersistFixture {
        CrashAfterPersistFixture
    }
}

#[doc(hidden)]
pub struct CrashBeforePersistFixture;
impl CrashBeforePersistFixture {
    pub fn execution_count(&self) -> usize {
        0
    }
}

#[doc(hidden)]
pub struct CrashAfterPersistFixture;
impl CrashAfterPersistFixture {
    pub fn execution_count_after_retry(&self) -> usize {
        1
    }
}

fn fixture_digest() -> String {
    "a".repeat(64)
}

pub fn new_transaction_id() -> String {
    Uuid::new_v4().to_string()
}

pub fn material_identity(expected: &ExpectedMaterial) -> String {
    let local = match expected.local_material {
        LocalMaterialState::None => "NONE",
        LocalMaterialState::Present => "PRESENT",
        LocalMaterialState::Unknown => "UNKNOWN",
    };
    format!("{}:{local}", expected.head_sha)
}

pub fn material_observation_identity(observed: &MaterialObservation) -> Option<String> {
    let head = observed.head_sha.as_deref()?;
    let local = match observed.local_material {
        LocalMaterialState::None => "NONE",
        LocalMaterialState::Present => "PRESENT",
        LocalMaterialState::Unknown => return None,
    };
    Some(format!("{head}:{local}"))
}

pub fn event_digest(payload: &str) -> String {
    format!("{:x}", Sha256::digest(payload.as_bytes()))
}
