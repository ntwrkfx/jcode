use jcode_project_executor::{ProjectExecutorSupervisor, SessionRunnability, SessionState};
use std::path::Path;
use std::process::Command;

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
    git(path, &["config", "user.name", "Retirement RED"]);
    git(
        path,
        &["config", "user.email", "retirement-red@example.invalid"],
    );
    std::fs::write(path.join("README.md"), "preserve me\n").unwrap();
    git(path, &["add", "README.md"]);
    git(path, &["commit", "-q", "-m", "base"]);
    git(path, &["rev-parse", "HEAD"])
}

#[tokio::test]
async fn logical_close_preserves_managed_harness_workspace_and_registration() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let workspace = root.path().join("managed-workspace");
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
    let before = git(&repo, &["worktree", "list", "--porcelain"]);

    let sessions = root.path().join("sessions");
    let id = "cccccccc-1111-4111-8111-cccccccccccc";
    let session_dir = sessions.join(id);
    std::fs::create_dir_all(session_dir.join("evidence")).unwrap();
    let revision = "cacacacacacacacacacacacacacacacacacacaca";
    let record = serde_json::json!({
        "schema_version": "project-executor-session/v2",
        "execution_id": id,
        "provider": "project-executor",
        "implementation": "jcode-derived-executor/v1",
        "implementation_revision": revision,
        "work_identity": "work:test:c-retirement-red",
        "repository": repo,
        "repository_path": repo,
        "base_sha": sha,
        "branch": "detached",
        "workspace": workspace,
        "worktree_binding": {
            "schema_version": "worktree-binding/v1",
            "mode": "MANAGED",
            "ownership": "HARNESS",
            "path": workspace,
            "repository": repo,
            "resolved_sha": sha,
            "access_mode": "write"
        },
        "lifecycle": "READY",
        "runnability": "RUNNABLE",
        "workspace_origin": "MANAGED",
        "expected_material": {"head_sha": sha, "local_material": "NONE"},
        "custody_assessment": {
            "state": "CONFIRMED",
            "assessed_at": 1,
            "executor_instance_id": "historical-executor",
            "generation": {"provider": "OLD", "token": "G1"}
        },
        "created_at": 1,
        "closed_at": null
    });
    std::fs::write(
        session_dir.join("session.json"),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();

    let supervisor =
        ProjectExecutorSupervisor::create_with_worktree_root(&sessions, root.path(), revision)
            .unwrap();
    let recovered = supervisor.inspect_session(id).await.unwrap();
    assert_eq!(recovered.runnability, SessionRunnability::Quarantined);

    let closed = supervisor.close_session(id).await.unwrap();
    assert_eq!(closed.state, SessionState::Closed);
    assert!(
        workspace.is_dir(),
        "logical close physically deleted managed source material"
    );
    assert_eq!(git(&repo, &["worktree", "list", "--porcelain"]), before);
    assert_eq!(
        std::fs::read_to_string(workspace.join("README.md")).unwrap(),
        "preserve me\n"
    );
}
