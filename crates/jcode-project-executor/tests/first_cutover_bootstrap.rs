use jcode_project_executor::{FirstCutoverObservation, assess_first_cutover};

fn observation() -> FirstCutoverObservation {
    FirstCutoverObservation {
        schema_version: "project-executor-first-cutover-observation/v1".into(),
        predecessor_revision: "a".repeat(40),
        successor_revision: "b".repeat(40),
        ready_session_count: 0,
        current_writer_custody_count: 0,
        unknown_session_count: 0,
        incomplete_transactional_upgrade: false,
    }
}

#[test]
fn first_cutover_is_ready_only_after_predecessor_work_is_drained() {
    let result = assess_first_cutover(&observation()).unwrap();
    assert_eq!(result.result, "READY_FOR_FIRST_CUTOVER");
    assert_eq!(result.authority_effect, "NONE");
    assert!(result.reason_codes.is_empty());
}

#[test]
fn first_cutover_blocks_ready_sessions_and_writer_custody() {
    let mut value = observation();
    value.ready_session_count = 1;
    value.current_writer_custody_count = 2;
    let result = assess_first_cutover(&value).unwrap();
    assert_eq!(result.result, "BLOCKED");
    assert!(
        result
            .reason_codes
            .contains(&"READY_SESSIONS_PRESENT".into())
    );
    assert!(
        result
            .reason_codes
            .contains(&"WRITER_CUSTODY_PRESENT".into())
    );
}

#[test]
fn first_cutover_blocks_unknown_state_and_incomplete_d_upgrade() {
    let mut value = observation();
    value.unknown_session_count = 1;
    value.incomplete_transactional_upgrade = true;
    let result = assess_first_cutover(&value).unwrap();
    assert_eq!(result.result, "BLOCKED");
    assert!(
        result
            .reason_codes
            .contains(&"UNKNOWN_SESSION_STATE".into())
    );
    assert!(
        result
            .reason_codes
            .contains(&"TRANSACTIONAL_UPGRADE_INCOMPLETE".into())
    );
}

#[test]
fn first_cutover_rejects_invalid_or_identical_revision_binding() {
    let mut invalid = observation();
    invalid.successor_revision = "not-a-sha".into();
    assert!(assess_first_cutover(&invalid).is_err());
    let mut same = observation();
    same.successor_revision = same.predecessor_revision.clone();
    assert!(assess_first_cutover(&same).is_err());
}
