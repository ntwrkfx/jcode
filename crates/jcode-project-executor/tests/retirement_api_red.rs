use jcode_project_executor::retirement::{
    RetirementDisposition, RetirementIntent, RetirementOutcome, RetirementReasonCode,
    WORKSPACE_RETIRE_EFFECT_CLASS,
};
use jcode_project_executor::{ProjectExecutorSupervisor, SessionRecord};
use std::path::{Path, PathBuf};
use std::process::Command;

const AUTH_DIGEST: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
const REVISION: &str = "cacacacacacacacacacacacacacacacacacacaca";
const RESOURCE: &str = "RES-RETIREMENT-TEST";

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn make_repo(path: &Path) -> String {
    std::fs::create_dir_all(path).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .arg(path)
            .status()
            .unwrap()
            .success()
    );
    git(path, &["config", "user.name", "Retirement API RED"]);
    git(
        path,
        &["config", "user.email", "retirement-api-red@example.invalid"],
    );
    std::fs::write(path.join("README.md"), "base\n").unwrap();
    git(path, &["add", "README.md"]);
    git(path, &["commit", "-q", "-m", "base"]);
    git(path, &["rev-parse", "HEAD"])
}

fn add_managed_worktree(repo: &Path, workspace: &Path, sha: &str) {
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["worktree", "add", "--detach"])
            .arg(workspace)
            .arg(sha)
            .status()
            .unwrap()
            .success()
    );
}

fn write_closed_record(
    sessions: &Path,
    execution_id: &str,
    repo: &Path,
    workspace: &Path,
    sha: &str,
    origin: &str,
    mode: &str,
    ownership: &str,
) {
    let session_dir = sessions.join(execution_id);
    std::fs::create_dir_all(session_dir.join("evidence")).unwrap();
    let record = serde_json::json!({
        "schema_version": "project-executor-session/v2",
        "execution_id": execution_id,
        "provider": "project-executor",
        "implementation": "jcode-derived-executor/v1",
        "implementation_revision": REVISION,
        "work_identity": "work:test:c-retirement-api-red",
        "repository": repo,
        "repository_path": repo,
        "base_sha": sha,
        "branch": "detached",
        "workspace": workspace,
        "worktree_binding": {
            "schema_version": "worktree-binding/v1",
            "mode": mode,
            "ownership": ownership,
            "path": workspace,
            "repository": repo,
            "resolved_sha": sha,
            "access_mode": "write"
        },
        "lifecycle": "CLOSED",
        "runnability": "QUARANTINED",
        "workspace_origin": origin,
        "expected_material": {"head_sha": sha, "local_material": "NONE"},
        "custody_assessment": null,
        "effect_authorization": null,
        "created_at": 1,
        "closed_at": 2
    });
    std::fs::write(
        session_dir.join("session.json"),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();
}

fn intent(id: &str, execution_id: &str, workspace: &Path, sha: &str) -> RetirementIntent {
    RetirementIntent {
        retirement_intent_id: id.to_owned(),
        work_id: "work:test:c-retirement-api-red".to_owned(),
        execution_id: execution_id.to_owned(),
        resource_identity: RESOURCE.to_owned(),
        workspace_identity: workspace.display().to_string(),
        candidate_revision: sha.to_owned(),
        effect_class: WORKSPACE_RETIRE_EFFECT_CLASS.to_owned(),
        authorization_digest: AUTH_DIGEST.to_owned(),
        expected_material: jcode_project_executor::ExpectedMaterial {
            head_sha: sha.to_owned(),
            local_material: jcode_project_executor::LocalMaterialState::None,
        },
        disposition: RetirementDisposition::DiscardCleanManagedWorkspace,
    }
}

async fn supervisor_for(sessions: &Path, worktree_root: &Path) -> ProjectExecutorSupervisor {
    ProjectExecutorSupervisor::create_with_worktree_root(sessions, worktree_root, REVISION).unwrap()
}

#[tokio::test]
async fn explicit_retirement_rejects_external_workspace() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo-external");
    let sha = make_repo(&repo);
    let workspace = root.path().join("external-workspace");
    add_managed_worktree(&repo, &workspace, &sha);
    let sessions = root.path().join("sessions-external");
    let execution_id = "dddddddd-1111-4111-8111-dddddddddddd";
    write_closed_record(
        &sessions,
        execution_id,
        &repo,
        &workspace,
        &sha,
        "EXTERNAL",
        "EXISTING",
        "EXTERNAL",
    );
    let supervisor = supervisor_for(&sessions, root.path()).await;

    let receipt = supervisor
        .retire_workspace(intent("retire-external", execution_id, &workspace, &sha))
        .await
        .unwrap();

    assert_eq!(receipt.outcome, RetirementOutcome::Denied);
    assert_eq!(
        receipt.reason,
        Some(RetirementReasonCode::NotManagedHarness)
    );
    assert!(workspace.is_dir());
}

#[tokio::test]
async fn explicit_retirement_rejects_legacy_workspace() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo-legacy");
    let sha = make_repo(&repo);
    let workspace = root.path().join("legacy-workspace");
    add_managed_worktree(&repo, &workspace, &sha);
    let sessions = root.path().join("sessions-legacy");
    let execution_id = "dddddddd-aaaa-4aaa-8aaa-dddddddddddd";
    write_closed_record(
        &sessions,
        execution_id,
        &repo,
        &workspace,
        &sha,
        "LEGACY",
        "EXISTING",
        "EXTERNAL",
    );
    let supervisor = supervisor_for(&sessions, root.path()).await;

    let receipt = supervisor
        .retire_workspace(intent("retire-legacy", execution_id, &workspace, &sha))
        .await
        .unwrap();

    assert_eq!(receipt.outcome, RetirementOutcome::Denied);
    assert_eq!(
        receipt.reason,
        Some(RetirementReasonCode::NotManagedHarness)
    );
    assert!(workspace.is_dir());
}

#[tokio::test]
async fn explicit_retirement_rejects_dirty_managed_workspace() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo-dirty");
    let sha = make_repo(&repo);
    let workspace = root.path().join("managed-dirty");
    add_managed_worktree(&repo, &workspace, &sha);
    std::fs::write(workspace.join("dirty.txt"), "do not delete\n").unwrap();
    let sessions = root.path().join("sessions-dirty");
    let execution_id = "eeeeeeee-2222-4222-8222-eeeeeeeeeeee";
    write_closed_record(
        &sessions,
        execution_id,
        &repo,
        &workspace,
        &sha,
        "MANAGED",
        "MANAGED",
        "HARNESS",
    );
    let supervisor = supervisor_for(&sessions, root.path()).await;

    let receipt = supervisor
        .retire_workspace(intent("retire-dirty", execution_id, &workspace, &sha))
        .await
        .unwrap();

    assert_eq!(receipt.outcome, RetirementOutcome::Denied);
    assert_eq!(receipt.reason, Some(RetirementReasonCode::MaterialPresent));
    assert!(workspace.join("dirty.txt").is_file());
}

#[tokio::test]
async fn missing_authorization_digest_denies_before_material_effect() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo-auth");
    let sha = make_repo(&repo);
    let workspace = root.path().join("managed-auth");
    add_managed_worktree(&repo, &workspace, &sha);
    let sessions = root.path().join("sessions-auth");
    let execution_id = "ffffffff-3333-4333-8333-ffffffffffff";
    write_closed_record(
        &sessions,
        execution_id,
        &repo,
        &workspace,
        &sha,
        "MANAGED",
        "MANAGED",
        "HARNESS",
    );
    let supervisor = supervisor_for(&sessions, root.path()).await;
    let mut request = intent("retire-auth", execution_id, &workspace, &sha);
    request.authorization_digest.clear();

    let receipt = supervisor.retire_workspace(request).await.unwrap();

    assert_eq!(receipt.outcome, RetirementOutcome::Denied);
    assert_eq!(
        receipt.reason,
        Some(RetirementReasonCode::AuthorizationMissing)
    );
    assert!(workspace.is_dir());
}

#[tokio::test]
async fn candidate_scope_mismatch_denies_without_material_effect() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo-scope");
    let sha = make_repo(&repo);
    let workspace = root.path().join("managed-scope");
    add_managed_worktree(&repo, &workspace, &sha);
    let sessions = root.path().join("sessions-scope");
    let execution_id = "99999999-6666-4666-8666-999999999999";
    write_closed_record(
        &sessions,
        execution_id,
        &repo,
        &workspace,
        &sha,
        "MANAGED",
        "MANAGED",
        "HARNESS",
    );
    let supervisor = supervisor_for(&sessions, root.path()).await;
    let mut request = intent("retire-scope", execution_id, &workspace, &sha);
    request.candidate_revision = "0000000000000000000000000000000000000000".to_owned();

    let receipt = supervisor.retire_workspace(request).await.unwrap();

    assert_eq!(receipt.outcome, RetirementOutcome::Denied);
    assert_eq!(receipt.reason, Some(RetirementReasonCode::ScopeMismatch));
    assert!(workspace.is_dir());
    assert_eq!(git(&workspace, &["rev-parse", "HEAD"]), sha);
}

#[tokio::test]
async fn clean_managed_retirement_is_idempotent_for_same_intent() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo-clean");
    let sha = make_repo(&repo);
    let workspace = root.path().join("managed-clean");
    add_managed_worktree(&repo, &workspace, &sha);
    let sessions = root.path().join("sessions-clean");
    let execution_id = "aaaaaaaa-4444-4444-8444-aaaaaaaaaaaa";
    write_closed_record(
        &sessions,
        execution_id,
        &repo,
        &workspace,
        &sha,
        "MANAGED",
        "MANAGED",
        "HARNESS",
    );
    let supervisor = supervisor_for(&sessions, root.path()).await;
    let request = intent("retire-clean", execution_id, &workspace, &sha);

    let first = supervisor.retire_workspace(request.clone()).await.unwrap();
    assert_eq!(first.outcome, RetirementOutcome::Retired);
    assert!(!workspace.exists());
    let after_first = git(&repo, &["worktree", "list", "--porcelain"]);
    assert!(!after_first.contains(&workspace.display().to_string()));

    let second = supervisor.retire_workspace(request).await.unwrap();
    assert_eq!(
        second, first,
        "duplicate intent must replay terminal receipt"
    );
}

#[tokio::test]
async fn absent_workspace_without_terminal_receipt_is_not_fabricated_as_retired() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo-absent");
    let sha = make_repo(&repo);
    let workspace = PathBuf::from(root.path().join("already-absent"));
    let sessions = root.path().join("sessions-absent");
    let execution_id = "bbbbbbbb-5555-4555-8555-bbbbbbbbbbbb";
    write_closed_record(
        &sessions,
        execution_id,
        &repo,
        &workspace,
        &sha,
        "MANAGED",
        "MANAGED",
        "HARNESS",
    );
    let supervisor = supervisor_for(&sessions, root.path()).await;

    let receipt = supervisor
        .retire_workspace(intent("retire-absent", execution_id, &workspace, &sha))
        .await
        .unwrap();

    assert_eq!(receipt.outcome, RetirementOutcome::AlreadyAbsentObserved);
    assert_ne!(receipt.outcome, RetirementOutcome::Retired);
}

#[test]
fn session_record_type_remains_non_retirement_authority() {
    let _ = std::mem::size_of::<SessionRecord>();
    assert_eq!(WORKSPACE_RETIRE_EFFECT_CLASS, "WORKSPACE_RETIRE");
}
