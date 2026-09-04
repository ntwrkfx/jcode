use crate::session::{CustodyAssessment, CustodyGenerationEvidence, CustodyState};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyScope {
    pub execution_id: String,
    pub collision_identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyAcquireRequest {
    pub scope: CustodyScope,
    pub executor_instance_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyGrant {
    pub scope: CustodyScope,
    pub executor_instance_id: String,
    pub generation: CustodyGenerationEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyEffectClaim {
    pub execution_id: String,
    pub collision_identity: String,
    pub executor_instance_id: String,
    pub generation: Option<CustodyGenerationEvidence>,
}

struct HeldGrant {
    grant: CustodyGrant,
    _lock: File,
}

pub struct LocalCustodyProvider {
    provider_name: String,
    lock_root: PathBuf,
    current: HashMap<String, HeldGrant>,
}

impl LocalCustodyProvider {
    pub fn new(provider_name: impl Into<String>, lock_root: impl AsRef<Path>) -> Result<Self> {
        let lock_root = lock_root.as_ref().to_path_buf();
        std::fs::create_dir_all(&lock_root).context("create custody lock root")?;
        Ok(Self {
            provider_name: provider_name.into(),
            lock_root,
            current: HashMap::new(),
        })
    }

    pub fn acquire(&mut self, request: CustodyAcquireRequest) -> Result<CustodyGrant> {
        if self.current.contains_key(&request.scope.collision_identity) {
            bail!("CUSTODY_NOT_CURRENT: collision scope already has a current grant");
        }
        let lock_path = self.lock_path(&request.scope.collision_identity);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)
            .context("open custody lock")?;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                bail!("CUSTODY_NOT_CURRENT: collision scope is held by another provider");
            }
            return Err(error).context("acquire custody lock");
        }
        let grant = CustodyGrant {
            scope: request.scope,
            executor_instance_id: request.executor_instance_id,
            generation: CustodyGenerationEvidence {
                provider: self.provider_name.clone(),
                token: Uuid::new_v4().to_string(),
            },
        };
        self.current.insert(
            grant.scope.collision_identity.clone(),
            HeldGrant {
                grant: grant.clone(),
                _lock: file,
            },
        );
        Ok(grant)
    }

    pub fn assess(&self, grant: &CustodyGrant) -> CustodyAssessment {
        if self.current_grant_matches(grant) {
            CustodyAssessment {
                state: CustodyState::Confirmed,
                assessed_at: now_millis(),
                executor_instance_id: Some(grant.executor_instance_id.clone()),
                generation: Some(grant.generation.clone()),
                evidence_ref: None,
            }
        } else {
            CustodyAssessment {
                state: CustodyState::NotHeld,
                assessed_at: now_millis(),
                executor_instance_id: None,
                generation: None,
                evidence_ref: None,
            }
        }
    }

    pub fn validate_effect(&self, claim: &CustodyEffectClaim) -> Result<()> {
        let generation = claim
            .generation
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("CUSTODY_GENERATION_MISSING"))?;
        let held = match self.current.get(&claim.collision_identity) {
            Some(held) => held,
            None if self
                .current
                .values()
                .any(|held| held.grant.scope.execution_id == claim.execution_id) =>
            {
                bail!("CUSTODY_COLLISION_IDENTITY_MISMATCH")
            }
            None => bail!("CUSTODY_NOT_CURRENT"),
        };
        if held.grant.scope.execution_id != claim.execution_id {
            bail!("CUSTODY_EXECUTION_ID_MISMATCH");
        }
        if held.grant.executor_instance_id != claim.executor_instance_id {
            bail!("CUSTODY_EXECUTOR_INSTANCE_MISMATCH");
        }
        if held.grant.generation != *generation {
            bail!("CUSTODY_GENERATION_STALE");
        }
        Ok(())
    }

    pub fn release(&mut self, grant: &CustodyGrant) -> Result<()> {
        if !self.current_grant_matches(grant) {
            bail!("CUSTODY_NOT_CURRENT: stale or mismatched release");
        }
        let held = self
            .current
            .remove(&grant.scope.collision_identity)
            .expect("current grant disappeared after validation");
        let rc = unsafe { libc::flock(held._lock.as_raw_fd(), libc::LOCK_UN) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error()).context("release custody lock");
        }
        Ok(())
    }

    fn current_grant_matches(&self, grant: &CustodyGrant) -> bool {
        self.current
            .get(&grant.scope.collision_identity)
            .map(|held| held.grant == *grant)
            .unwrap_or(false)
    }

    fn lock_path(&self, collision_identity: &str) -> PathBuf {
        self.lock_root
            .join(format!("{:016x}.lock", stable_key(collision_identity)))
    }
}

fn stable_key(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}
