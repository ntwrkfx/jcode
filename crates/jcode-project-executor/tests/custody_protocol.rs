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

fn claim(
    grant: &jcode_project_executor::custody::CustodyGrant,
) -> CustodyEffectClaim {
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
    assert!(provider.validate_effect(&effect).is_err());
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
    mismatched.generation.as_mut().unwrap().token.push_str("-wrong");
    assert!(provider.validate_effect(&mismatched).is_err());
}

#[test]
fn effect_is_bound_to_execution_and_collision_scope_at_effect_time() {
    let root = tempfile::tempdir().unwrap();
    let mut provider = LocalCustodyProvider::new("TEST_LOCAL_FLOCK", root.path()).unwrap();
    let grant = acquire(&mut provider, "executor-A");
    let mut wrong_collision = claim(&grant);
    wrong_collision.collision_identity = "device-1|git-common-1|other-workspace".to_owned();
    assert!(provider.validate_effect(&wrong_collision).is_err());

    let mut wrong_execution = claim(&grant);
    wrong_execution.execution_id = "execution-2".to_owned();
    assert!(provider.validate_effect(&wrong_execution).is_err());

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
    assert!(provider
        .acquire(CustodyAcquireRequest {
            scope: scope(),
            executor_instance_id: "executor-B".to_owned(),
        })
        .is_err());
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
