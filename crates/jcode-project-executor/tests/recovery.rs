use jcode_project_executor::{
    CustodyAssessment, CustodyGenerationEvidence, CustodyState, LocalMaterialState,
    RecoveryEvidence, RecoveryReasonCode, SessionLifecycle, SessionRunnability, WorkspaceOrigin,
    decode_session_record, derive_runnability,
};
use serde_json::json;

fn legacy_ready() -> serde_json::Value {
    json!({
        "schema_version": "project-executor-session/v1",
        "execution_id": "11111111-1111-4111-8111-111111111111",
        "provider": "project-executor",
        "implementation": "jcode-derived-executor/v1",
        "implementation_revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "work_identity": "local:test:legacy",
        "repository": "ntwrkfx/example",
        "repository_path": "/tmp/repo",
        "base_sha": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "branch": "main",
        "workspace": "/tmp/workspace",
        "state": "READY",
        "created_at": 1,
        "closed_at": null
    })
}

#[test]
fn schema_legacy_ready_translates_fail_closed() {
    let decoded = decode_session_record(legacy_ready()).unwrap();
    assert!(decoded.translated_from_legacy);
    assert_eq!(decoded.record.lifecycle, SessionLifecycle::Ready);
    assert_eq!(decoded.record.runnability, SessionRunnability::Quarantined);
    assert_eq!(decoded.record.workspace_origin, WorkspaceOrigin::Legacy);
    assert_eq!(
        decoded.record.expected_material.local_material,
        LocalMaterialState::Unknown
    );
    assert_eq!(
        decoded.record.custody_assessment.as_ref().unwrap().state,
        CustodyState::Unknown
    );
    assert_eq!(
        decoded.initial_reason_codes,
        vec![RecoveryReasonCode::LegacyRecordRequiresReconciliation]
    );
}

#[test]
fn schema_persisted_confirmed_custody_resets_to_unknown() {
    let mut value = legacy_ready();
    value["schema_version"] = json!("project-executor-session/v2");
    value.as_object_mut().unwrap().remove("state");
    value["lifecycle"] = json!("READY");
    value["runnability"] = json!("RUNNABLE");
    value["workspace_origin"] = json!("MANAGED");
    value["expected_material"] = json!({
        "head_sha": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "local_material": "NONE"
    });
    value["custody_assessment"] = json!({
        "state": "CONFIRMED",
        "assessed_at": 7,
        "executor_instance_id": "old-instance",
        "generation": {"provider": "PROJECT_EXECUTOR_FENCE", "token": "old-token"},
        "evidence_ref": "receipt:old"
    });
    let decoded = decode_session_record(value).unwrap();
    let custody = decoded.record.custody_assessment.unwrap();
    assert_eq!(custody.state, CustodyState::Unknown);
    assert_eq!(custody.executor_instance_id, None);
    assert_eq!(custody.generation, None);
}

fn custody(state: CustodyState, instance: Option<&str>, token: Option<&str>) -> CustodyAssessment {
    CustodyAssessment {
        state,
        assessed_at: 10,
        executor_instance_id: instance.map(str::to_owned),
        generation: token.map(|token| CustodyGenerationEvidence {
            provider: "PROJECT_EXECUTOR_FENCE".to_owned(),
            token: token.to_owned(),
        }),
        evidence_ref: None,
    }
}

fn evidence() -> RecoveryEvidence {
    RecoveryEvidence {
        lifecycle: SessionLifecycle::Ready,
        workspace_origin: WorkspaceOrigin::Managed,
        schema_understood: true,
        revision_matches: true,
        binding_matches: true,
        material_matches: true,
        material_sufficiently_known: true,
        custody: custody(CustodyState::Confirmed, Some("current"), Some("token")),
        current_executor_instance_id: "current".to_owned(),
        write_custody_required: true,
    }
}

#[test]
fn runnability_complete_current_evidence_is_runnable() {
    let decision = derive_runnability(&evidence());
    assert_eq!(decision.result, SessionRunnability::Runnable);
    assert!(decision.reason_codes.is_empty());
}

#[test]
fn runnability_historical_or_unknown_custody_never_passes() {
    for state in [CustodyState::Unknown, CustodyState::NotHeld] {
        let mut input = evidence();
        input.custody = custody(state, None, None);
        let decision = derive_runnability(&input);
        assert_eq!(decision.result, SessionRunnability::Quarantined);
        assert!(!decision.reason_codes.is_empty());
    }
}

#[test]
fn runnability_wrong_executor_instance_or_missing_fence_never_passes() {
    let mut wrong = evidence();
    wrong.custody = custody(CustodyState::Confirmed, Some("old"), Some("token"));
    assert_eq!(
        derive_runnability(&wrong).result,
        SessionRunnability::Quarantined
    );

    let mut no_token = evidence();
    no_token.custody = custody(CustodyState::Confirmed, Some("current"), None);
    assert_eq!(
        derive_runnability(&no_token).result,
        SessionRunnability::Quarantined
    );
}

#[test]
fn runnability_material_unknown_or_changed_never_passes() {
    let mut unknown = evidence();
    unknown.material_sufficiently_known = false;
    let decision = derive_runnability(&unknown);
    assert_eq!(decision.result, SessionRunnability::Quarantined);
    assert_eq!(
        decision.reason_codes,
        vec![RecoveryReasonCode::MaterialStateUnknown]
    );

    let mut changed = evidence();
    changed.material_matches = false;
    let decision = derive_runnability(&changed);
    assert_eq!(decision.result, SessionRunnability::Quarantined);
    assert_eq!(
        decision.reason_codes,
        vec![RecoveryReasonCode::MaterialChanged]
    );
}

#[test]
fn runnability_revision_and_binding_mismatch_fail_closed() {
    let mut revision = evidence();
    revision.revision_matches = false;
    assert_eq!(
        derive_runnability(&revision).reason_codes,
        vec![RecoveryReasonCode::RevisionMismatch]
    );

    let mut binding = evidence();
    binding.binding_matches = false;
    assert_eq!(
        derive_runnability(&binding).reason_codes,
        vec![RecoveryReasonCode::WorkspaceBindingMismatch]
    );
}

#[test]
fn runnability_read_session_does_not_require_writer_custody() {
    let mut input = evidence();
    input.write_custody_required = false;
    input.custody = custody(CustodyState::Unknown, None, None);
    assert_eq!(
        derive_runnability(&input).result,
        SessionRunnability::Runnable
    );
}

fn make_git_repo(path: &std::path::Path) -> String {
    use std::process::Command;
    std::fs::create_dir_all(path).unwrap();
    assert!(
        Command::new("git")
            .arg("init")
            .arg("-q")
            .arg(path)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["config", "user.email", "test@example.invalid"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["config", "user.name", "Test"])
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(path.join("tracked.txt"), "one\n").unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["add", "tracked.txt"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["commit", "-qm", "base"])
            .status()
            .unwrap()
            .success()
    );
    String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_owned()
}

#[test]
fn material_clean_git_workspace_is_exact_and_none() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let head = make_git_repo(&repo);
    let observed = jcode_project_executor::observe_git_material(&repo);
    assert_eq!(observed.head_sha.as_deref(), Some(head.as_str()));
    assert_eq!(observed.local_material, LocalMaterialState::None);
}

#[test]
fn material_tracked_and_untracked_changes_are_present() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    make_git_repo(&repo);
    std::fs::write(repo.join("tracked.txt"), "changed\n").unwrap();
    let observed = jcode_project_executor::observe_git_material(&repo);
    assert_eq!(observed.local_material, LocalMaterialState::Present);

    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["checkout", "--", "tracked.txt"])
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(repo.join("untracked.txt"), "new\n").unwrap();
    let observed = jcode_project_executor::observe_git_material(&repo);
    assert_eq!(observed.local_material, LocalMaterialState::Present);
}

#[test]
fn material_missing_or_non_git_workspace_is_unknown() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing");
    let observed = jcode_project_executor::observe_git_material(&missing);
    assert_eq!(observed.head_sha, None);
    assert_eq!(observed.local_material, LocalMaterialState::Unknown);

    let plain = root.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    let observed = jcode_project_executor::observe_git_material(&plain);
    assert_eq!(observed.local_material, LocalMaterialState::Unknown);
}

#[test]
fn recovery_summary_is_deterministic_by_session_identity() {
    use jcode_project_executor::{
        MaterialObservation, RecoveryHealth, RecoverySummary, SessionRecoveryReceipt,
    };
    fn receipt(id: &str, result: SessionRunnability) -> SessionRecoveryReceipt {
        SessionRecoveryReceipt {
            schema_version: "SessionRecoveryReceipt/v1".to_owned(),
            session_id: id.to_owned(),
            expected_executor_revision: "a".repeat(40),
            observed_executor_revision: "a".repeat(40),
            expected_workspace: format!("/tmp/{id}"),
            observed_workspace: Some(format!("/tmp/{id}")),
            expected_material: jcode_project_executor::ExpectedMaterial {
                head_sha: "b".repeat(40),
                local_material: LocalMaterialState::None,
            },
            observed_material: MaterialObservation {
                head_sha: Some("b".repeat(40)),
                local_material: LocalMaterialState::None,
                observed_at: 10,
            },
            custody: custody(CustodyState::Unknown, None, None),
            result,
            reason_codes: if result == SessionRunnability::Runnable {
                vec![]
            } else {
                vec![RecoveryReasonCode::CustodyIndeterminate]
            },
            observed_at: 10,
        }
    }
    let a = jcode_project_executor::summarize_recovery(vec![
        receipt("b", SessionRunnability::Quarantined),
        receipt("a", SessionRunnability::Runnable),
        receipt("c", SessionRunnability::FailedRecovery),
    ]);
    let b = jcode_project_executor::summarize_recovery(vec![
        receipt("c", SessionRunnability::FailedRecovery),
        receipt("a", SessionRunnability::Runnable),
        receipt("b", SessionRunnability::Quarantined),
    ]);
    assert_eq!(a, b);
    assert_eq!(a.health, RecoveryHealth::Degraded);
    assert_eq!(a.runnable, 1);
    assert_eq!(a.quarantined, 1);
    assert_eq!(a.failed_recovery, 1);
    assert_eq!(
        a.receipts
            .iter()
            .map(|x| x.session_id.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b", "c"]
    );
    let _: RecoverySummary = a;
}

#[test]
fn unsupported_schema_with_safe_identity_is_quarantined_not_rejected() {
    let mut value = legacy_ready();
    value["schema_version"] = json!("project-executor-session/v99");
    let decoded = decode_session_record(value).unwrap();
    assert!(!decoded.schema_understood);
    assert!(!decoded.translated_from_legacy);
    assert_eq!(
        decoded.record.schema_version,
        "project-executor-session/v99"
    );
    assert_eq!(decoded.record.lifecycle, SessionLifecycle::Ready);
    assert_eq!(decoded.record.runnability, SessionRunnability::Quarantined);
    assert_eq!(
        decoded.record.custody_assessment.as_ref().unwrap().state,
        CustodyState::Unknown
    );
    assert_eq!(
        decoded.initial_reason_codes,
        vec![RecoveryReasonCode::SessionSchemaUnsupported]
    );
}

#[test]
fn unsupported_schema_without_safe_identity_is_rejected() {
    let value = json!({
        "schema_version": "project-executor-session/v99",
        "provider": "project-executor",
        "state": "READY"
    });
    assert!(decode_session_record(value).is_err());
}
