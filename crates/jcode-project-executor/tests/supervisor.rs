use jcode_project_executor::{
    AccessMode, ProjectExecutorSupervisor, SessionCreateRequest, SessionState, WorktreeMode,
    WorktreeOwnership,
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
    git(path, &["commit", "-q", "-m", "first"]);
    git(path, &["rev-parse", "HEAD"])
}

fn request(id: &str, repo: &Path, sha: &str) -> SessionCreateRequest {
    SessionCreateRequest {
        execution_id: id.to_owned(),
        work_identity: format!("local:test:{id}"),
        repository: repo.display().to_string(),
        base_sha: sha.to_owned(),
        worktree_path: None,
        access_mode: None,
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

#[tokio::test]
async fn git_remains_usable_inside_isolated_worktree() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let supervisor = ProjectExecutorSupervisor::create(
        root.path().join("sessions"),
        "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
    )
    .unwrap();
    let id = "88888888-8888-4888-8888-888888888888";
    supervisor
        .create_session(request(id, &repo, &sha))
        .await
        .unwrap();
    let process = supervisor
        .start_process(
            id,
            vec![
                "/bin/sh".into(),
                "-c".into(),
                format!(
                    "test ! -e '{}/README.md' && git status --porcelain",
                    repo.display()
                ),
            ],
            None,
        )
        .await
        .unwrap();
    let state = supervisor
        .wait_process(id, &process.process_id, None)
        .await
        .unwrap();
    assert_eq!(
        state,
        jcode_project_executor::ExecutionProcessState::Exited { code: Some(0) }
    );
    let output = supervisor
        .read_process(id, &process.process_id, 0, 1024)
        .await
        .unwrap();
    assert_eq!(output.data, "");
    supervisor.close_session(id).await.unwrap();
}

#[tokio::test]
async fn existing_worktree_binding_preserves_external_tree_and_enforces_writer_lock() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let external = root.path().join("external");
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["worktree", "add", "--detach"])
            .arg(&external)
            .arg(&sha)
            .status()
            .unwrap()
            .success()
    );
    let sessions = root.path().join("sessions");
    let supervisor = ProjectExecutorSupervisor::create_with_worktree_root(
        &sessions,
        root.path(),
        "abababababababababababababababababababab",
    )
    .unwrap();
    let writer_a = "aaaaaaaa-1111-4111-8111-111111111111";
    let mut write_request = request(writer_a, &repo, &sha);
    write_request.worktree_path = Some(external.display().to_string());
    write_request.access_mode = Some(AccessMode::Write);
    let created = supervisor.create_session(write_request).await.unwrap();
    assert_eq!(created.worktree_binding.mode, WorktreeMode::Existing);
    assert_eq!(
        created.worktree_binding.ownership,
        WorktreeOwnership::External
    );
    assert_eq!(created.worktree_binding.access_mode, AccessMode::Write);
    assert_eq!(
        created.worktree_binding.path,
        external.display().to_string()
    );

    let writer_b = "bbbbbbbb-2222-4222-8222-222222222222";
    let mut conflicting = request(writer_b, &repo, &sha);
    conflicting.worktree_path = Some(external.display().to_string());
    conflicting.access_mode = Some(AccessMode::Write);
    let error = supervisor
        .create_session(conflicting.clone())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("WORKTREE_BUSY"));

    let reader = "cccccccc-3333-4333-8333-333333333333";
    let mut read_request = request(reader, &repo, &sha);
    read_request.worktree_path = Some(external.display().to_string());
    read_request.access_mode = Some(AccessMode::Read);
    supervisor.create_session(read_request).await.unwrap();

    supervisor.close_session(writer_a).await.unwrap();
    assert!(
        external.is_dir(),
        "EXISTING+EXTERNAL close deleted source tree"
    );
    let reopened = supervisor.create_session(conflicting).await.unwrap();
    assert_eq!(reopened.worktree_binding.access_mode, AccessMode::Write);
}

#[tokio::test]
async fn writer_can_bind_different_tree_while_existing_tree_is_locked() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let a = root.path().join("tree-a");
    let b = root.path().join("tree-b");
    for tree in [&a, &b] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["worktree", "add", "--detach"])
                .arg(tree)
                .arg(&sha)
                .status()
                .unwrap()
                .success()
        );
    }
    let supervisor = ProjectExecutorSupervisor::create_with_worktree_root(
        root.path().join("sessions"),
        root.path(),
        "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd",
    )
    .unwrap();
    for (id, tree) in [
        ("dddddddd-4444-4444-8444-444444444444", &a),
        ("eeeeeeee-5555-4555-8555-555555555555", &b),
    ] {
        let mut req = request(id, &repo, &sha);
        req.worktree_path = Some(tree.display().to_string());
        req.access_mode = Some(AccessMode::Write);
        supervisor.create_session(req).await.unwrap();
    }
}

#[tokio::test]
async fn read_binding_is_read_only_and_worktree_discovery_reports_writer() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let external = root.path().join("external");
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["worktree", "add", "--detach"])
            .arg(&external)
            .arg(&sha)
            .status()
            .unwrap()
            .success()
    );
    let supervisor = ProjectExecutorSupervisor::create_with_worktree_root(
        root.path().join("sessions"),
        root.path(),
        "efefefefefefefefefefefefefefefefefefefef",
    )
    .unwrap();
    let writer = "ffffffff-6666-4666-8666-666666666666";
    let mut write_request = request(writer, &repo, &sha);
    write_request.worktree_path = Some(external.display().to_string());
    write_request.access_mode = Some(AccessMode::Write);
    supervisor.create_session(write_request).await.unwrap();
    let inspected = supervisor
        .inspect_worktree(external.to_str().unwrap())
        .await
        .unwrap();
    assert_eq!(inspected.writer.as_deref(), Some(writer));
    assert!(
        supervisor
            .list_worktrees()
            .await
            .unwrap()
            .iter()
            .any(|item| item.path == external.display().to_string())
    );

    let reader = "12121212-7777-4777-8777-777777777777";
    let mut read_request = request(reader, &repo, &sha);
    read_request.worktree_path = Some(external.display().to_string());
    read_request.access_mode = Some(AccessMode::Read);
    supervisor.create_session(read_request).await.unwrap();
    let process = supervisor
        .start_process(
            reader,
            vec!["/usr/bin/touch".into(), "should-not-exist".into()],
            None,
        )
        .await
        .unwrap();
    let state = supervisor
        .wait_process(reader, &process.process_id, None)
        .await
        .unwrap();
    assert_ne!(
        state,
        jcode_project_executor::ExecutionProcessState::Exited { code: Some(0) }
    );
    assert!(!external.join("should-not-exist").exists());
}

#[tokio::test]
async fn existing_worktree_from_independent_clone_with_same_origin_is_admitted() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let remote = root.path().join("remote.git");
    assert!(
        Command::new("git")
            .args(["clone", "--bare"])
            .arg(&repo)
            .arg(&remote)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["remote", "add", "origin"])
            .arg(&remote)
            .status()
            .unwrap()
            .success()
    );
    let clone = root.path().join("independent-clone");
    assert!(
        Command::new("git")
            .arg("clone")
            .arg(&remote)
            .arg(&clone)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        Command::new("git")
            .arg("-C")
            .arg(&clone)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
        format!("{sha}\n").as_bytes()
    );

    let supervisor = ProjectExecutorSupervisor::create_with_worktree_root(
        root.path().join("sessions"),
        root.path(),
        "3434343434343434343434343434343434343434",
    )
    .unwrap();
    let mut req = request("34343434-8888-4888-8888-888888888888", &repo, &sha);
    req.worktree_path = Some(clone.display().to_string());
    req.access_mode = Some(AccessMode::Read);
    let inspection = supervisor.create_session(req).await.unwrap();
    assert_eq!(inspection.worktree_binding.mode, WorktreeMode::Existing);
    assert_eq!(
        inspection.worktree_binding.ownership,
        WorktreeOwnership::External
    );
}

#[tokio::test]
async fn rejected_external_binding_does_not_poison_execution_id() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let invalid = root.path().join("not-a-repository");
    std::fs::create_dir(&invalid).unwrap();
    let sessions = root.path().join("sessions");
    let supervisor = ProjectExecutorSupervisor::create_with_worktree_root(
        &sessions,
        root.path(),
        "5656565656565656565656565656565656565656",
    )
    .unwrap();
    let id = "56565656-9999-4999-8999-999999999999";
    let mut rejected = request(id, &repo, &sha);
    rejected.worktree_path = Some(invalid.display().to_string());
    rejected.access_mode = Some(AccessMode::Read);
    assert!(supervisor.create_session(rejected).await.is_err());
    assert!(!sessions.join(id).exists());

    let external = root.path().join("valid");
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["worktree", "add", "--detach"])
            .arg(&external)
            .arg(&sha)
            .status()
            .unwrap()
            .success()
    );
    let mut accepted = request(id, &repo, &sha);
    accepted.worktree_path = Some(external.display().to_string());
    accepted.access_mode = Some(AccessMode::Read);
    supervisor.create_session(accepted).await.unwrap();
}

#[tokio::test]
async fn incompatible_ready_session_is_quarantined_without_deleting_workspace() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let sessions = root.path().join("sessions");
    let id = "13131313-8888-4888-8888-888888888888";
    let old_revision = "1111111111111111111111111111111111111111";
    let new_revision = "2222222222222222222222222222222222222222";

    let first = ProjectExecutorSupervisor::create(&sessions, old_revision).unwrap();
    let created = first.create_session(request(id, &repo, &sha)).await.unwrap();
    std::fs::write(Path::new(&created.workspace).join("preserve.txt"), "valuable\n").unwrap();
    drop(first);

    let recovered = ProjectExecutorSupervisor::create(&sessions, new_revision).unwrap();
    let inspection = recovered.inspect_session(id).await.unwrap();
    assert_eq!(inspection.state, SessionState::Ready);
    assert_eq!(inspection.implementation_revision, old_revision);
    assert!(Path::new(&created.workspace).join("preserve.txt").is_file());
}
