use jcode_project_executor::custody::{CustodyAcquireRequest, CustodyScope, LocalCustodyProvider};
use jcode_project_executor::{
    AccessMode, ProjectExecutorSupervisor, SessionCreateRequest, WorktreeMode, WorktreeOwnership,
};
use std::path::{Path, PathBuf};
use std::process::Command;

const AUTH_DIGEST: &str = "efefefefefefefefefefefefefefefefefefefefefefefefefefefefefefefef";
const REVISION: &str = "cacacacacacacacacacacacacacacacacacacaca";

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
    git(path, &["config", "user.name", "Close Adversarial RED"]);
    git(path, &["config", "user.email", "close-red@example.invalid"]);
    std::fs::write(path.join("README.md"), "base\n").unwrap();
    git(path, &["add", "README.md"]);
    git(path, &["commit", "-q", "-m", "base"]);
    git(path, &["rev-parse", "HEAD"])
}

fn add_worktree(repo: &Path, workspace: &Path, sha: &str) {
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

fn request(id: &str, repo: &Path, workspace: &Path, sha: &str) -> SessionCreateRequest {
    SessionCreateRequest {
        execution_id: id.to_owned(),
        work_identity: format!("work:test:c-close:{id}"),
        repository: repo.display().to_string(),
        base_sha: sha.to_owned(),
        worktree_path: Some(workspace.display().to_string()),
        access_mode: Some(AccessMode::Write),
        authorization_digest: Some(AUTH_DIGEST.to_owned()),
    }
}

fn competing_provider(sessions: &Path) -> LocalCustodyProvider {
    LocalCustodyProvider::new(
        "C_CLOSE_ADVERSARIAL_COMPETITOR",
        sessions.join(".worktree-locks"),
    )
    .unwrap()
}

fn competing_request(id: &str, workspace: &Path) -> CustodyAcquireRequest {
    CustodyAcquireRequest {
        scope: CustodyScope {
            execution_id: id.to_owned(),
            collision_identity: workspace.canonicalize().unwrap().display().to_string(),
        },
        executor_instance_id: "competing-executor".to_owned(),
    }
}

#[tokio::test]
async fn close_persistence_failure_must_not_release_current_custody_first() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo-order");
    let sha = make_repo(&repo);
    let workspace = root.path().join("external-order");
    add_worktree(&repo, &workspace, &sha);
    let sessions = root.path().join("sessions-order");
    let id = "c1000000-0000-4000-8000-000000000001";
    let supervisor =
        ProjectExecutorSupervisor::create_with_worktree_root(&sessions, root.path(), REVISION)
            .unwrap();
    let created = supervisor
        .create_session(request(id, &repo, &workspace, &sha))
        .await
        .unwrap();
    assert_eq!(created.worktree_binding.mode, WorktreeMode::Existing);
    assert_eq!(
        created.worktree_binding.ownership,
        WorktreeOwnership::External
    );

    let session_json = sessions.join(id).join("session.json");
    std::fs::remove_file(&session_json).unwrap();
    std::fs::create_dir(&session_json).unwrap();

    assert!(
        supervisor.close_session(id).await.is_err(),
        "forced persistence failure must fail close"
    );
    let mut competitor = competing_provider(&sessions);
    assert!(
        competitor
            .acquire(competing_request(id, &workspace))
            .is_err(),
        "custody was released before CLOSED persistence succeeded"
    );
    assert!(workspace.is_dir());
}

fn mark_closed_with_release_unresolved(session_json: &Path) {
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(session_json).unwrap()).unwrap();
    record["lifecycle"] = serde_json::json!("CLOSED");
    record["runnability"] = serde_json::json!("QUARANTINED");
    record["closed_at"] = serde_json::json!(2);
    record["custody_assessment"] = serde_json::json!({
        "state": "UNKNOWN",
        "assessed_at": 2,
        "executor_instance_id": null,
        "generation": null,
        "evidence_ref": "CUSTODY_RELEASE_UNRESOLVED"
    });
    std::fs::write(session_json, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
}

#[tokio::test]
async fn closed_with_release_unresolved_cannot_be_silently_settled_on_retry() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo-release");
    let sha = make_repo(&repo);
    let workspace = root.path().join("external-release");
    add_worktree(&repo, &workspace, &sha);
    let sessions = root.path().join("sessions-release");
    let id = "c2000000-0000-4000-8000-000000000002";
    {
        let first =
            ProjectExecutorSupervisor::create_with_worktree_root(&sessions, root.path(), REVISION)
                .unwrap();
        first
            .create_session(request(id, &repo, &workspace, &sha))
            .await
            .unwrap();
    }
    let session_json = sessions.join(id).join("session.json");
    mark_closed_with_release_unresolved(&session_json);

    let recovered =
        ProjectExecutorSupervisor::create_with_worktree_root(&sessions, root.path(), REVISION)
            .unwrap();
    assert!(
        recovered.close_session(id).await.is_err(),
        "CLOSED + CUSTODY_RELEASE_UNRESOLVED must not be returned as a silently settled close"
    );
    assert!(workspace.is_dir());
}

#[test]
fn close_source_must_persist_release_unresolved_state_on_release_failure() {
    let source = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/supervisor.rs"),
    )
    .unwrap();
    assert!(
        source.contains("CUSTODY_RELEASE_UNRESOLVED"),
        "close must durably represent custody-release failure rather than hiding it behind CLOSED"
    );
}

#[test]
fn destructive_state_persistence_requires_file_and_parent_directory_sync() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut combined = String::new();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        if entry.path().extension().and_then(|v| v.to_str()) == Some("rs") {
            combined.push_str(&std::fs::read_to_string(entry.path()).unwrap());
        }
    }
    let sync_calls = combined.matches("sync_all()").count();
    assert!(
        sync_calls >= 2,
        "C claims durable destructive state but production source lacks file + parent-directory sync_all calls"
    );
}
