use jcode_project_executor::{
    ResumeIntent, TransactionalUpgradeRequest, TransactionalUpgradeState,
    TransactionalUpgradeStore, UpgradePhase, event_digest,
};

fn ready_state() -> TransactionalUpgradeState {
    let request = TransactionalUpgradeRequest::new(
        "transaction-d-state",
        "work:d-state",
        "attempt-d-state",
        "execution-d-state",
        "/tmp/d-state-workspace",
        "executor-v1",
        "G1",
        "1111111111111111111111111111111111111111",
        "2222222222222222222222222222222222222222",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:NONE",
        7,
    )
    .unwrap();
    let mut state = TransactionalUpgradeState::new(request);
    state.quiesce().unwrap();
    state.persist_transition().unwrap();
    state.release_predecessor_generation().unwrap();
    state.start_successor("executor-v2").unwrap();
    state.recover_successor(true).unwrap();
    state.acquire_successor_generation("G2").unwrap();
    state
        .verify_successor(
            Some("2222222222222222222222222222222222222222"),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:NONE"),
            Some(7),
        )
        .unwrap();
    state
}

fn intent(state: &TransactionalUpgradeState, event: &str, payload: &str) -> ResumeIntent {
    ResumeIntent::new(
        state.request.work_id.clone(),
        state.request.attempt_id.clone(),
        state.request.execution_id.clone(),
        state.request.checkpoint_generation,
        event,
        event_digest(payload),
        state.successor_generation().to_owned(),
        state.successor_executor_instance_id.clone().unwrap(),
        state.request.expected_successor_runtime_identity.clone(),
        state.request.expected_material_identity.clone(),
    )
    .unwrap()
}

#[test]
fn durable_resume_persists_event_consumption_and_successor_checkpoint_before_return() {
    let root = tempfile::tempdir().unwrap();
    let store = TransactionalUpgradeStore::new(root.path().join("state.json"));
    let mut state = ready_state();
    let payload = "event payload";
    let resume = intent(&state, "event-1", payload);

    state
        .accept_resume_durably(&store, &resume, payload)
        .unwrap();
    assert_eq!(state.phase, UpgradePhase::Complete);
    assert!(state.admissions_open);
    assert_eq!(state.continuation_count, 1);
    assert_eq!(state.request.checkpoint_generation, 8);

    let recovered = store.load().unwrap();
    assert_eq!(recovered, state);
    assert_eq!(
        recovered.consumed_events.get("event-1"),
        Some(&event_digest(payload))
    );
    assert!(recovered.admissions_open);
}

#[test]
fn payload_digest_mismatch_is_denied_without_state_transition_or_durable_consumption() {
    let root = tempfile::tempdir().unwrap();
    let store = TransactionalUpgradeStore::new(root.path().join("state.json"));
    let mut state = ready_state();
    let before = state.clone();
    let resume = intent(&state, "event-1", "expected payload");

    let error = state
        .accept_resume_durably(&store, &resume, "different payload")
        .unwrap_err();
    assert!(error.to_string().contains("D_RESUME_EVENT_DIGEST_MISMATCH"));
    assert_eq!(state, before);
    assert!(!store.path().exists());
}

#[test]
fn unknown_or_mismatched_successor_verification_fails_closed() {
    for (runtime, material, checkpoint) in [
        (
            None,
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:NONE"),
            Some(7),
        ),
        (
            Some("2222222222222222222222222222222222222222"),
            None,
            Some(7),
        ),
        (
            Some("2222222222222222222222222222222222222222"),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb:NONE"),
            Some(7),
        ),
        (
            Some("2222222222222222222222222222222222222222"),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:NONE"),
            None,
        ),
    ] {
        let request = TransactionalUpgradeRequest::new(
            "transaction-d-unknown",
            "work:d",
            "attempt:d",
            "execution:d",
            "/tmp/d-workspace",
            "executor-v1",
            "G1",
            "1111111111111111111111111111111111111111",
            "2222222222222222222222222222222222222222",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:NONE",
            7,
        )
        .unwrap();
        let mut state = TransactionalUpgradeState::new(request);
        state.quiesce().unwrap();
        state.persist_transition().unwrap();
        state.release_predecessor_generation().unwrap();
        state.start_successor("executor-v2").unwrap();
        state.recover_successor(true).unwrap();
        state.acquire_successor_generation("G2").unwrap();
        assert!(
            state
                .verify_successor(runtime, material, checkpoint)
                .is_err()
        );
        assert_eq!(state.phase, UpgradePhase::Failed);
        assert!(!state.admissions_open);
        assert!(!state.successor_authority_current);
    }
}

#[test]
fn historical_g1_cannot_be_reacquired_as_successor_generation() {
    let request = TransactionalUpgradeRequest::new(
        "transaction-d-g1",
        "work:d",
        "attempt:d",
        "execution:d",
        "/tmp/d-workspace",
        "executor-v1",
        "G1",
        "1111111111111111111111111111111111111111",
        "2222222222222222222222222222222222222222",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:NONE",
        7,
    )
    .unwrap();
    let mut state = TransactionalUpgradeState::new(request);
    state.quiesce().unwrap();
    state.persist_transition().unwrap();
    state.release_predecessor_generation().unwrap();
    state.start_successor("executor-v2").unwrap();
    state.recover_successor(true).unwrap();
    assert!(state.acquire_successor_generation("G1").is_err());
    assert_eq!(state.phase, UpgradePhase::Failed);
    assert!(!state.admissions_open);
}
