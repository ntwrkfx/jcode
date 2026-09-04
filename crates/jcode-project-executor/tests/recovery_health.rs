use jcode_project_executor::{
    ExecutorCommand, ExecutorRequest, ProjectExecutorSupervisor, SessionRunnability,
    handle_executor_request,
};
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
    git(path, &["config", "user.name", "Executor Test"]);
    git(path, &["config", "user.email", "executor@example.invalid"]);
    std::fs::write(path.join("README.md"), "first\n").unwrap();
    git(path, &["add", "README.md"]);
    git(path, &["commit", "-qm", "first"]);
    git(path, &["rev-parse", "HEAD"])
}

async fn health(supervisor: &ProjectExecutorSupervisor) -> serde_json::Value {
    let response = handle_executor_request(
        supervisor,
        ExecutorRequest {
            protocol: "project-executor/v1".into(),
            id: "health".into(),
            command: ExecutorCommand::Health,
        },
    )
    .await;
    assert!(response.ok, "health failed: {:?}", response.error);
    response.result.unwrap()
}

#[tokio::test]
async fn health_separates_serving_from_healthy_recovery() {
    let root = tempfile::tempdir().unwrap();
    let supervisor = ProjectExecutorSupervisor::create_with_worktree_root(
        root.path().join("sessions"),
        root.path(),
        "1111111111111111111111111111111111111111",
    )
    .unwrap();
    let value = health(&supervisor).await;
    assert_eq!(value["service"]["state"], "SERVING");
    assert_eq!(value["recovery"]["state"], "HEALTHY");
    assert_eq!(value["recovery"]["runnable"], 0);
    assert_eq!(value["recovery"]["quarantined"], 0);
    assert_eq!(value["recovery"]["failed_recovery"], 0);
}

#[tokio::test]
async fn degraded_recovery_does_not_make_service_unavailable() {
    let root = tempfile::tempdir().unwrap();
    let sessions = root.path().join("sessions");
    let id = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    let session_dir = sessions.join(id);
    let workspace = session_dir.join("workspace");
    let sha = make_repo(&workspace);
    std::fs::create_dir_all(session_dir.join("evidence")).unwrap();
    let revision = "2222222222222222222222222222222222222222";
    let legacy = serde_json::json!({
        "schema_version": "project-executor-session/v1",
        "execution_id": id,
        "provider": "project-executor",
        "implementation": "jcode-derived-executor/v1",
        "implementation_revision": revision,
        "work_identity": "local:test:health-legacy",
        "repository": workspace,
        "repository_path": workspace,
        "base_sha": sha,
        "branch": "main",
        "workspace": workspace,
        "state": "READY",
        "created_at": 1,
        "closed_at": null
    });
    std::fs::write(
        session_dir.join("session.json"),
        serde_json::to_vec_pretty(&legacy).unwrap(),
    )
    .unwrap();
    let supervisor =
        ProjectExecutorSupervisor::create_with_worktree_root(&sessions, root.path(), revision)
            .unwrap();
    assert_eq!(
        supervisor.inspect_session(id).await.unwrap().runnability,
        SessionRunnability::Quarantined
    );
    let value = health(&supervisor).await;
    assert_eq!(value["service"]["state"], "SERVING");
    assert_eq!(value["recovery"]["state"], "DEGRADED");
    assert_eq!(value["recovery"]["quarantined"], 1);
}

#[tokio::test]
async fn malformed_only_recovery_is_failed_while_service_still_serves() {
    let root = tempfile::tempdir().unwrap();
    let sessions = root.path().join("sessions");
    let bad = sessions.join("eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee");
    std::fs::create_dir_all(&bad).unwrap();
    std::fs::write(
        bad.join("session.json"),
        r#"{"schema_version":"project-executor-session/v99","state":"READY"}"#,
    )
    .unwrap();
    let supervisor = ProjectExecutorSupervisor::create_with_worktree_root(
        &sessions,
        root.path(),
        "3333333333333333333333333333333333333333",
    )
    .unwrap();
    let value = health(&supervisor).await;
    assert_eq!(value["service"]["state"], "SERVING");
    assert_eq!(value["recovery"]["state"], "FAILED");
    assert_eq!(value["recovery"]["failed_recovery"], 1);
}
