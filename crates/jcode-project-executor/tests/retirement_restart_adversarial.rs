use jcode_project_executor::effect::EffectAuthorizationBinding;
use jcode_project_executor::retirement::{
    RetirementDisposition, RetirementIntent, RetirementOutcome, RetirementReasonCode,
    WORKSPACE_RETIRE_EFFECT_CLASS,
};
use jcode_project_executor::{ExpectedMaterial, LocalMaterialState, ProjectExecutorSupervisor};
use std::path::Path;
use std::process::Command;

const REVISION: &str = "cacacacacacacacacacacacacacacacacacacaca";
const DIGEST: &str = "abababababababababababababababababababababababababababababababab";

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn make_repo(repo: &Path) -> String {
    std::fs::create_dir_all(repo).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .arg(repo)
            .status()
            .unwrap()
            .success()
    );
    git(repo, &["config", "user.name", "C Restart RED"]);
    git(repo, &["config", "user.email", "c-restart@example.invalid"]);
    std::fs::write(repo.join("README.md"), "base\n").unwrap();
    git(repo, &["add", "README.md"]);
    git(repo, &["commit", "-q", "-m", "base"]);
    git(repo, &["rev-parse", "HEAD"])
}

#[tokio::test]
async fn persisted_intent_plus_absent_workspace_without_receipt_is_ambiguous_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let workspace = root.path().join("managed");
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["worktree", "add", "--detach"])
            .arg(&workspace)
            .arg(&sha)
            .status()
            .unwrap()
            .success()
    );
    let sessions = root.path().join("sessions");
    let execution_id = "cb000000-0000-4000-8000-00000000000b";
    let session_dir = sessions.join(execution_id);
    std::fs::create_dir_all(session_dir.join("evidence/retirement/intents")).unwrap();
    let workspace_identity = workspace.canonicalize().unwrap().display().to_string();
    let record = serde_json::json!({
        "schema_version":"project-executor-session/v2", "execution_id":execution_id,
        "provider":"project-executor", "implementation":"jcode-derived-executor/v1",
        "implementation_revision":REVISION, "work_identity":"work:test:c-restart",
        "repository":repo, "repository_path":repo, "base_sha":sha, "branch":"detached",
        "workspace":workspace, "worktree_binding":{"schema_version":"worktree-binding/v1","mode":"MANAGED","ownership":"HARNESS","path":workspace,"repository":repo,"resolved_sha":sha,"access_mode":"write"},
        "lifecycle":"CLOSED", "runnability":"QUARANTINED", "workspace_origin":"MANAGED",
        "expected_material":{"head_sha":sha,"local_material":"NONE"}, "custody_assessment":null,
        "effect_authorization":null, "created_at":1, "closed_at":2
    });
    std::fs::write(
        session_dir.join("session.json"),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();
    let binding = EffectAuthorizationBinding {
        work_id: "work:test:c-restart".into(),
        execution_id: execution_id.into(),
        resource_identity: "RES-C-RESTART".into(),
        workspace_identity: workspace_identity.clone(),
        candidate_revision: sha.clone(),
        effect_class: WORKSPACE_RETIRE_EFFECT_CLASS.into(),
        authorization_digest: DIGEST.into(),
    };
    let intent = RetirementIntent {
        retirement_intent_id: "retire-crash-restart".into(),
        work_id: binding.work_id.clone(),
        execution_id: execution_id.into(),
        resource_identity: binding.resource_identity.clone(),
        workspace_identity,
        candidate_revision: sha.clone(),
        effect_class: WORKSPACE_RETIRE_EFFECT_CLASS.into(),
        authorization_digest: DIGEST.into(),
        authorization_binding: Some(binding),
        expected_material: ExpectedMaterial {
            head_sha: sha.clone(),
            local_material: LocalMaterialState::None,
        },
        disposition: RetirementDisposition::DiscardCleanManagedWorkspace,
    };
    std::fs::write(
        session_dir.join("evidence/retirement/intents/retire-crash-restart.json"),
        serde_json::to_vec_pretty(&intent).unwrap(),
    )
    .unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["worktree", "remove"])
            .arg(&workspace)
            .status()
            .unwrap()
            .success()
    );
    let supervisor =
        ProjectExecutorSupervisor::create_with_worktree_root(&sessions, root.path(), REVISION)
            .unwrap();
    let receipt = supervisor.retire_workspace(intent).await.unwrap();
    assert_eq!(receipt.outcome, RetirementOutcome::OutcomeAmbiguous);
    assert_eq!(receipt.reason, Some(RetirementReasonCode::OutcomeAmbiguous));
}

#[tokio::test]
async fn invalid_execution_id_is_rejected_before_retirement_evidence_path_resolution() {
    let root = tempfile::tempdir().unwrap();
    let sessions = root.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let supervisor =
        ProjectExecutorSupervisor::create_with_worktree_root(&sessions, root.path(), REVISION)
            .unwrap();
    let binding = EffectAuthorizationBinding {
        work_id: "work:test:c-invalid-id".into(),
        execution_id: "../../escape".into(),
        resource_identity: "RES-C-INVALID".into(),
        workspace_identity: root.path().join("never-used").display().to_string(),
        candidate_revision: "dededededededededededededededededededede".into(),
        effect_class: WORKSPACE_RETIRE_EFFECT_CLASS.into(),
        authorization_digest: DIGEST.into(),
    };
    let intent = RetirementIntent {
        retirement_intent_id: "retire-invalid-id".into(),
        work_id: binding.work_id.clone(),
        execution_id: binding.execution_id.clone(),
        resource_identity: binding.resource_identity.clone(),
        workspace_identity: binding.workspace_identity.clone(),
        candidate_revision: binding.candidate_revision.clone(),
        effect_class: binding.effect_class.clone(),
        authorization_digest: DIGEST.into(),
        authorization_binding: Some(binding),
        expected_material: ExpectedMaterial {
            head_sha: "dededededededededededededededededededede".into(),
            local_material: LocalMaterialState::None,
        },
        disposition: RetirementDisposition::DiscardCleanManagedWorkspace,
    };
    let error = supervisor.retire_workspace(intent).await.unwrap_err();
    assert!(error.to_string().contains("execution_id must be a UUID"));
    assert!(!root.path().join("escape").exists());
}

#[tokio::test]
async fn wrong_resource_identity_is_denied_before_retirement_effect() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo-resource");
    let sha = make_repo(&repo);
    let workspace = root.path().join("managed-resource");
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["worktree", "add", "--detach"])
            .arg(&workspace)
            .arg(&sha)
            .status()
            .unwrap()
            .success()
    );
    let sessions = root.path().join("sessions-resource");
    let execution_id = "cb000000-0000-4000-8000-00000000000c";
    let session_dir = sessions.join(execution_id);
    std::fs::create_dir_all(&session_dir).unwrap();
    let workspace_identity = workspace.canonicalize().unwrap().display().to_string();
    let record = serde_json::json!({
        "schema_version":"project-executor-session/v2", "execution_id":execution_id,
        "provider":"project-executor", "implementation":"jcode-derived-executor/v1",
        "implementation_revision":REVISION, "work_identity":"work:test:c-resource",
        "repository":"RESOURCE-ACTUAL", "repository_path":repo, "base_sha":sha, "branch":"detached",
        "workspace":workspace, "worktree_binding":{"schema_version":"worktree-binding/v1","mode":"MANAGED","ownership":"HARNESS","path":workspace,"repository":"RESOURCE-ACTUAL","resolved_sha":sha,"access_mode":"write"},
        "lifecycle":"CLOSED", "runnability":"QUARANTINED", "workspace_origin":"MANAGED",
        "expected_material":{"head_sha":sha,"local_material":"NONE"}, "custody_assessment":null,
        "effect_authorization":null, "created_at":1, "closed_at":2
    });
    std::fs::write(
        session_dir.join("session.json"),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();
    let binding = EffectAuthorizationBinding {
        work_id: "work:test:c-resource".into(),
        execution_id: execution_id.into(),
        resource_identity: "RESOURCE-WRONG".into(),
        workspace_identity: workspace_identity.clone(),
        candidate_revision: sha.clone(),
        effect_class: WORKSPACE_RETIRE_EFFECT_CLASS.into(),
        authorization_digest: DIGEST.into(),
    };
    let intent = RetirementIntent {
        retirement_intent_id: "retire-wrong-resource".into(),
        work_id: binding.work_id.clone(),
        execution_id: execution_id.into(),
        resource_identity: binding.resource_identity.clone(),
        workspace_identity,
        candidate_revision: sha.clone(),
        effect_class: WORKSPACE_RETIRE_EFFECT_CLASS.into(),
        authorization_digest: DIGEST.into(),
        authorization_binding: Some(binding),
        expected_material: ExpectedMaterial {
            head_sha: sha,
            local_material: LocalMaterialState::None,
        },
        disposition: RetirementDisposition::DiscardCleanManagedWorkspace,
    };
    let supervisor =
        ProjectExecutorSupervisor::create_with_worktree_root(&sessions, root.path(), REVISION)
            .unwrap();
    let receipt = supervisor.retire_workspace(intent).await.unwrap();
    assert_eq!(receipt.outcome, RetirementOutcome::Denied);
    assert_eq!(receipt.reason, Some(RetirementReasonCode::ScopeMismatch));
    assert!(workspace.exists());
}
