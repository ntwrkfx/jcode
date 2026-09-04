//! Increment D pre-authorization RED surface.
//! Production implementation is intentionally absent at the accepted C parent.

use jcode_project_executor::{
    ProjectExecutorSupervisor, ResumeIntent, TransactionalUpgradeRequest, UpgradePhase,
};

const G1: &str = "G1";
const G2: &str = "G2";

#[test]
fn replacement_phase_order_is_explicit_and_fail_closed() {
    assert_eq!(
        UpgradePhase::ORDER,
        [
            UpgradePhase::Quiesce,
            UpgradePhase::Persist,
            UpgradePhase::ReleaseOldGeneration,
            UpgradePhase::StartSuccessor,
            UpgradePhase::Recover,
            UpgradePhase::AcquireNewGeneration,
            UpgradePhase::VerifyRuntimeAndMaterial,
            UpgradePhase::ResumeAdmission,
        ]
    );
}

#[test]
fn predecessor_generation_cannot_survive_successor_boundary() {
    let request = TransactionalUpgradeRequest::fixture(G1, G2);
    assert!(request.predecessor_invalid_before_successor_effect());
    assert!(request.stale_generation_rejected_after_successor());
}

#[test]
fn successor_failure_or_unknown_keeps_admissions_closed() {
    let request = TransactionalUpgradeRequest::fixture(G1, G2);
    for failed in request.required_failure_frontier() {
        assert!(!failed.resume_admission_allowed());
    }
}

#[test]
fn valid_recoverable_work_can_reacquire_fresh_successor_generation() {
    let request = TransactionalUpgradeRequest::fixture(G1, G2);
    let recovered = request.successful_recovery_fixture();
    assert_ne!(recovered.predecessor_generation(), recovered.successor_generation());
    assert_eq!(recovered.successor_generation(), G2);
    assert!(recovered.resume_admission_allowed());
}

#[test]
fn resume_intent_is_generation_checkpoint_event_and_material_bound() {
    let intent = ResumeIntent::fixture();
    assert!(intent.matches_current_checkpoint());
    assert!(intent.matches_current_generation());
    assert!(intent.matches_runtime_identity());
    assert!(intent.matches_material_identity());
    assert!(intent.external_event_digest_matches());
}

#[test]
fn duplicate_stale_and_concurrent_resume_are_denied() {
    let supervisor = ProjectExecutorSupervisor::transactional_upgrade_fixture();
    let intent = ResumeIntent::fixture();
    assert!(supervisor.resume_once(intent.clone()).is_ok());
    assert!(supervisor.resume_once(intent.clone()).is_err());
    assert!(supervisor.resume_with_stale_checkpoint(intent.clone()).is_err());
    assert!(supervisor.resume_with_stale_generation(intent.clone()).is_err());
    assert!(supervisor.resume_concurrently(intent).accepted_count() <= 1);
}

#[test]
fn crash_boundaries_do_not_duplicate_continuation_effect() {
    let supervisor = ProjectExecutorSupervisor::transactional_upgrade_fixture();
    assert_eq!(supervisor.crash_before_event_persist_fixture().execution_count(), 0);
    assert_eq!(supervisor.crash_after_event_persist_before_execution_fixture().execution_count_after_retry(), 1);
}

#[test]
fn old_executor_continuation_is_denied_after_successor_advances() {
    let supervisor = ProjectExecutorSupervisor::transactional_upgrade_fixture();
    assert!(supervisor.old_executor_continuation_fixture().is_err());
}
