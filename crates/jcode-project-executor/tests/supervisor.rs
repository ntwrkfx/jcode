use jcode_project_executor::{ProjectExecutorSupervisor, SessionCreateRequest, SessionState};
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
    git(path, &["commit", "-q", "-m", "first"]);
    git(path, &["rev-parse", "HEAD"])
}

fn request(id: &str, repo: &Path, sha: &str) -> SessionCreateRequest {
    SessionCreateRequest {
        execution_id: id.to_owned(),
        work_identity: format!("local:test:{id}"),
        repository: repo.display().to_string(),
        base_sha: sha.to_owned(),
    }
}

#[tokio::test]
async fn session_create_pins_sha_and_recovers_from_catalog() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let sessions = root.path().join("sessions");
    let id = "11111111-1111-4111-8111-111111111111";
    let revision = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    let first = ProjectExecutorSupervisor::create(&sessions, revision).unwrap();
    let created = first
        .create_session(request(id, &repo, &sha))
        .await
        .unwrap();
    assert_eq!(created.execution_id, id);
    assert_eq!(created.base_sha, sha);
    assert_eq!(created.head_sha, sha);
    assert!(created.clean);
    assert_eq!(created.state, SessionState::Ready);
    assert!(Path::new(&created.workspace).is_dir());
    drop(first);

    let recovered = ProjectExecutorSupervisor::create(&sessions, revision).unwrap();
    let inspection = recovered.inspect_session(id).await.unwrap();
    assert_eq!(inspection.execution_id, id);
    assert_eq!(inspection.base_sha, sha);
    assert_eq!(inspection.head_sha, sha);
    assert_eq!(inspection.implementation_revision, revision);
}

#[tokio::test]
async fn sessions_namespace_process_ids_and_reject_cross_session_access() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let supervisor = ProjectExecutorSupervisor::create(
        root.path().join("sessions"),
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    )
    .unwrap();
    let a = "22222222-2222-4222-8222-222222222222";
    let b = "33333333-3333-4333-8333-333333333333";
    supervisor
        .create_session(request(a, &repo, &sha))
        .await
        .unwrap();
    supervisor
        .create_session(request(b, &repo, &sha))
        .await
        .unwrap();

    let process = supervisor
        .start_process(a, vec!["printf".into(), "alpha".into()], None)
        .await
        .unwrap();
    assert!(process.process_id.starts_with(&format!("{a}:p-")));
    let error = supervisor
        .read_process(b, &process.process_id, 0, 32)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("does not belong to execution"));
    supervisor
        .wait_process(a, &process.process_id, None)
        .await
        .unwrap();
    let output = supervisor
        .read_process(a, &process.process_id, 0, 32)
        .await
        .unwrap();
    assert_eq!(output.data, "alpha");
}

#[tokio::test]
async fn close_removes_worktree_preserves_record_and_evidence() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let sessions = root.path().join("sessions");
    let supervisor =
        ProjectExecutorSupervisor::create(&sessions, "cccccccccccccccccccccccccccccccccccccccc")
            .unwrap();
    let id = "44444444-4444-4444-8444-444444444444";
    let created = supervisor
        .create_session(request(id, &repo, &sha))
        .await
        .unwrap();
    let process = supervisor
        .start_process(id, vec!["printf".into(), "evidence".into()], None)
        .await
        .unwrap();
    supervisor
        .wait_process(id, &process.process_id, None)
        .await
        .unwrap();
    let workspace = created.workspace.clone();

    let closed = supervisor.close_session(id).await.unwrap();
    assert_eq!(closed.state, SessionState::Closed);
    assert!(!Path::new(&workspace).exists());
    assert!(sessions.join(id).join("session.json").is_file());
    assert!(
        sessions
            .join(id)
            .join("evidence")
            .join("processes")
            .is_dir()
    );
    let error = supervisor
        .start_process(id, vec!["true".into()], None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("closed"));
}
