use jcode_project_executor::custody::{
    CustodyAcquireRequest, CustodyEffectClaim, CustodyScope, LocalCustodyProvider,
};

fn scope() -> CustodyScope {
    CustodyScope {
        execution_id: "execution-1".to_owned(),
        collision_identity: "device-1|git-common-1|workspace-1".to_owned(),
    }
}

fn acquire(
    provider: &mut LocalCustodyProvider,
    executor_instance_id: &str,
) -> jcode_project_executor::custody::CustodyGrant {
    provider
        .acquire(CustodyAcquireRequest {
            scope: scope(),
            executor_instance_id: executor_instance_id.to_owned(),
        })
        .unwrap()
}

fn claim(grant: &jcode_project_executor::custody::CustodyGrant) -> CustodyEffectClaim {
    CustodyEffectClaim {
        execution_id: grant.scope.execution_id.clone(),
        collision_identity: grant.scope.collision_identity.clone(),
        executor_instance_id: grant.executor_instance_id.clone(),
        generation: Some(grant.generation.clone()),
    }
}

#[test]
fn provider_issues_opaque_generation_and_does_not_reuse_it() {
    let root = tempfile::tempdir().unwrap();
    let mut provider = LocalCustodyProvider::new("TEST_LOCAL_FLOCK", root.path()).unwrap();
    let g1 = acquire(&mut provider, "executor-A");
    assert!(!g1.generation.token.is_empty());
    assert!(!g1.generation.token.contains("execution-1"));
    assert!(!g1.generation.token.contains("workspace-1"));
    provider.release(&g1).unwrap();
    let g2 = acquire(&mut provider, "executor-B");
    assert_ne!(g1.generation, g2.generation);
}

#[test]
fn persisted_generation_evidence_is_not_current_custody() {
    let root = tempfile::tempdir().unwrap();
    let persisted = {
        let mut provider = LocalCustodyProvider::new("TEST_LOCAL_FLOCK", root.path()).unwrap();
        let g1 = acquire(&mut provider, "executor-A");
        claim(&g1)
    };
    let provider = LocalCustodyProvider::new("TEST_LOCAL_FLOCK", root.path()).unwrap();
    assert!(provider.validate_effect(&persisted).is_err());
}

#[test]
fn stale_g1_is_rejected_after_successor_g2() {
    let root = tempfile::tempdir().unwrap();
    let mut provider = LocalCustodyProvider::new("TEST_LOCAL_FLOCK", root.path()).unwrap();
    let g1 = acquire(&mut provider, "executor-A");
    let old_claim = claim(&g1);
    assert!(provider.assess(&g1).is_confirmed());
    assert!(provider.validate_effect(&old_claim).is_ok());
    provider.release(&g1).unwrap();
    let g2 = acquire(&mut provider, "executor-B");
    assert!(provider.validate_effect(&old_claim).is_err());
    assert!(provider.validate_effect(&claim(&g2)).is_ok());
}

#[test]
fn effect_rejects_wrong_executor_instance() {
    let root = tempfile::tempdir().unwrap();
    let mut provider = LocalCustodyProvider::new("TEST_LOCAL_FLOCK", root.path()).unwrap();
    let grant = acquire(&mut provider, "executor-A");
    let mut effect = claim(&grant);
    effect.executor_instance_id = "executor-B".to_owned();
    let error = provider.validate_effect(&effect).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("CUSTODY_EXECUTOR_INSTANCE_MISMATCH")
    );
}

#[test]
fn effect_rejects_missing_or_mismatched_generation() {
    let root = tempfile::tempdir().unwrap();
    let mut provider = LocalCustodyProvider::new("TEST_LOCAL_FLOCK", root.path()).unwrap();
    let grant = acquire(&mut provider, "executor-A");
    let mut missing = claim(&grant);
    missing.generation = None;
    assert!(provider.validate_effect(&missing).is_err());

    let mut mismatched = claim(&grant);
    mismatched
        .generation
        .as_mut()
        .unwrap()
        .token
        .push_str("-wrong");
    assert!(provider.validate_effect(&mismatched).is_err());
}

#[test]
fn effect_is_bound_to_execution_and_collision_scope_at_effect_time() {
    let root = tempfile::tempdir().unwrap();
    let mut provider = LocalCustodyProvider::new("TEST_LOCAL_FLOCK", root.path()).unwrap();
    let grant = acquire(&mut provider, "executor-A");
    let mut wrong_collision = claim(&grant);
    wrong_collision.collision_identity = "device-1|git-common-1|other-workspace".to_owned();
    let error = provider.validate_effect(&wrong_collision).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("CUSTODY_COLLISION_IDENTITY_MISMATCH")
    );

    let mut wrong_execution = claim(&grant);
    wrong_execution.execution_id = "execution-2".to_owned();
    let error = provider.validate_effect(&wrong_execution).unwrap_err();
    assert!(error.to_string().contains("CUSTODY_EXECUTION_ID_MISMATCH"));

    let prior = claim(&grant);
    assert!(provider.assess(&grant).is_confirmed());
    provider.release(&grant).unwrap();
    assert!(provider.validate_effect(&prior).is_err());
}

#[test]
fn concurrent_same_scope_acquire_is_rejected_until_current_grant_is_released() {
    let root = tempfile::tempdir().unwrap();
    let mut provider = LocalCustodyProvider::new("TEST_LOCAL_FLOCK", root.path()).unwrap();
    let g1 = acquire(&mut provider, "executor-A");
    assert!(
        provider
            .acquire(CustodyAcquireRequest {
                scope: scope(),
                executor_instance_id: "executor-B".to_owned(),
            })
            .is_err()
    );
    assert!(provider.validate_effect(&claim(&g1)).is_ok());
}

#[test]
fn stale_release_cannot_revoke_successor_generation() {
    let root = tempfile::tempdir().unwrap();
    let mut provider = LocalCustodyProvider::new("TEST_LOCAL_FLOCK", root.path()).unwrap();
    let g1 = acquire(&mut provider, "executor-A");
    provider.release(&g1).unwrap();
    let g2 = acquire(&mut provider, "executor-B");
    assert!(provider.release(&g1).is_err());
    assert!(provider.validate_effect(&claim(&g2)).is_ok());
}

#[test]
fn independent_provider_instances_fence_same_collision_scope() {
    let root = tempfile::tempdir().unwrap();
    let mut first = LocalCustodyProvider::new("TEST_LOCAL_FLOCK", root.path()).unwrap();
    let mut second = LocalCustodyProvider::new("TEST_LOCAL_FLOCK", root.path()).unwrap();
    let g1 = acquire(&mut first, "executor-A");
    assert!(
        second
            .acquire(CustodyAcquireRequest {
                scope: scope(),
                executor_instance_id: "executor-B".to_owned(),
            })
            .is_err()
    );
    first.release(&g1).unwrap();
    let g2 = acquire(&mut second, "executor-B");
    assert_ne!(g1.generation, g2.generation);
    assert!(second.validate_effect(&claim(&g2)).is_ok());
}

#[test]
fn provider_is_fenced_by_legacy_worktree_lock_file_identity() {
    use std::fs::OpenOptions;
    use std::os::fd::AsRawFd;

    fn legacy_key(value: &str) -> u64 {
        let mut hash = 0xcbf29ce484222325u64;
        for byte in value.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }

    let root = tempfile::tempdir().unwrap();
    let collision = scope().collision_identity;
    let lock_path = root
        .path()
        .join(format!("{:016x}.lock", legacy_key(&collision)));
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(lock_path)
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let mut provider = LocalCustodyProvider::new("TEST_LOCAL_FLOCK", root.path()).unwrap();
    let error = provider
        .acquire(CustodyAcquireRequest {
            scope: CustodyScope {
                execution_id: "execution-2".to_owned(),
                collision_identity: collision,
            },
            executor_instance_id: "executor-B".to_owned(),
        })
        .unwrap_err();
    assert!(error.to_string().contains("CUSTODY_NOT_CURRENT"));
}
