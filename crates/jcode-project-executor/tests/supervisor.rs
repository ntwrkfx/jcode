use jcode_project_executor::{
    AccessMode, ProjectExecutorSupervisor, SessionCreateRequest, SessionRunnability, SessionState,
    WorktreeMode, WorktreeOwnership,
};
use std::path::Path;
use std::process::Command;

const AUTH_DIGEST: &str = "abababababababababababababababababababababababababababababababab";

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
        authorization_digest: Some(AUTH_DIGEST.to_owned()),
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
        .start_process(
            a,
            vec!["printf".into(), "alpha".into()],
            None,
            Some(AUTH_DIGEST.to_owned()),
        )
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
async fn close_preserves_worktree_registration_record_and_evidence() {
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
        .start_process(
            id,
            vec!["printf".into(), "evidence".into()],
            None,
            Some(AUTH_DIGEST.to_owned()),
        )
        .await
        .unwrap();
    supervisor
        .wait_process(id, &process.process_id, None)
        .await
        .unwrap();
    let workspace = created.workspace.clone();

    let closed = supervisor.close_session(id).await.unwrap();
    assert_eq!(closed.state, SessionState::Closed);
    assert!(Path::new(&workspace).is_dir());
    let registered = git(&repo, &["worktree", "list", "--porcelain"]);
    assert!(registered.contains(&workspace));
    assert!(sessions.join(id).join("session.json").is_file());
    assert!(
        sessions
            .join(id)
            .join("evidence")
            .join("processes")
            .is_dir()
    );
    let error = supervisor
        .start_process(id, vec!["true".into()], None, Some(AUTH_DIGEST.to_owned()))
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
            Some(AUTH_DIGEST.to_owned()),
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
    read_request.authorization_digest = None;
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
    read_request.authorization_digest = None;
    supervisor.create_session(read_request).await.unwrap();
    let process = supervisor
        .start_process(
            reader,
            vec!["/usr/bin/touch".into(), "should-not-exist".into()],
            None,
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
    let created = first
        .create_session(request(id, &repo, &sha))
        .await
        .unwrap();
    std::fs::write(
        Path::new(&created.workspace).join("preserve.txt"),
        "valuable\n",
    )
    .unwrap();
    drop(first);

    let recovered = ProjectExecutorSupervisor::create(&sessions, new_revision).unwrap();
    let inspection = recovered.inspect_session(id).await.unwrap();
    assert_eq!(inspection.state, SessionState::Ready);
    assert_eq!(inspection.implementation_revision, old_revision);
    assert!(Path::new(&created.workspace).join("preserve.txt").is_file());
    let discovered = recovered
        .inspect_worktree(&created.workspace)
        .await
        .unwrap();
    assert_eq!(discovered.writer, None);
}

#[tokio::test]
async fn matching_ready_missing_workspace_is_recorded_failed_instead_of_aborting_startup() {
    let root = tempfile::tempdir().unwrap();
    let sessions = root.path().join("sessions");
    let id = "41414141-4141-4141-8141-414141414141";
    let revision = "3333333333333333333333333333333333333333";
    let session_dir = sessions.join(id);
    let workspace = session_dir.join("workspace");
    std::fs::create_dir_all(session_dir.join("evidence")).unwrap();

    let record = serde_json::json!({
        "schema_version": "project-executor-session/v1",
        "execution_id": id,
        "provider": "project-executor",
        "implementation": "jcode-derived-executor/v1",
        "implementation_revision": revision,
        "work_identity": "local:test:missing-recovery",
        "repository": root.path().join("repo"),
        "repository_path": root.path().join("repo"),
        "base_sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "branch": "main",
        "workspace": workspace,
        "worktree_binding": {
            "schema_version": "worktree-binding/v1",
            "mode": "MANAGED",
            "ownership": "HARNESS",
            "path": workspace,
            "repository": root.path().join("repo"),
            "resolved_sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "access_mode": "write"
        },
        "state": "READY",
        "created_at": 1,
        "closed_at": null
    });
    std::fs::write(
        session_dir.join("session.json"),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();

    let recovered =
        ProjectExecutorSupervisor::create_with_worktree_root(&sessions, root.path(), revision)
            .unwrap();
    let inspection = recovered.inspect_session(id).await.unwrap();
    assert_eq!(inspection.state, SessionState::Ready);
    assert_eq!(inspection.lifecycle, SessionState::Ready);
    assert_eq!(inspection.runnability, SessionRunnability::FailedRecovery);
    assert_eq!(inspection.implementation_revision, revision);

    let persisted: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(session_dir.join("session.json")).unwrap())
            .unwrap();
    assert_eq!(persisted["schema_version"], "project-executor-session/v2");
    assert_eq!(persisted["lifecycle"], "READY");
    assert_eq!(persisted["runnability"], "FAILED_RECOVERY");
    assert!(persisted.get("state").is_none());

    let receipts_dir = session_dir.join("evidence/recovery");
    let receipts = std::fs::read_dir(&receipts_dir)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(receipts.len(), 1);
    let recovery: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(receipts[0].path()).unwrap()).unwrap();
    assert_eq!(recovery["schema_version"], "SessionRecoveryReceipt/v1");
    assert_eq!(recovery["result"], "FAILED_RECOVERY");
    assert_eq!(
        recovery["reason_codes"],
        serde_json::json!(["READY_WORKSPACE_MISSING"])
    );
    assert_eq!(recovered.recovery_summary().failed_recovery, 1);
    let error = recovered
        .start_process(id, vec!["true".into()], None, Some(AUTH_DIGEST.to_owned()))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("execution is not runnable"));
}

#[tokio::test]
async fn recovery_failure_is_isolated_and_receipts_are_sorted() {
    let root = tempfile::tempdir().unwrap();
    let sessions = root.path().join("sessions");
    let revision = "7777777777777777777777777777777777777777";

    let present_id = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    let present_dir = sessions.join(present_id);
    let present_workspace = present_dir.join("workspace");
    let present_sha = make_repo(&present_workspace);
    std::fs::create_dir_all(present_dir.join("evidence")).unwrap();
    let present = serde_json::json!({
        "schema_version": "project-executor-session/v1",
        "execution_id": present_id,
        "provider": "project-executor",
        "implementation": "jcode-derived-executor/v1",
        "implementation_revision": revision,
        "work_identity": "local:test:present-legacy",
        "repository": present_workspace,
        "repository_path": present_workspace,
        "base_sha": present_sha,
        "branch": "main",
        "workspace": present_workspace,
        "state": "READY",
        "created_at": 2,
        "closed_at": null
    });
    std::fs::write(
        present_dir.join("session.json"),
        serde_json::to_vec_pretty(&present).unwrap(),
    )
    .unwrap();

    let missing_id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let missing_dir = sessions.join(missing_id);
    let missing_workspace = missing_dir.join("workspace");
    std::fs::create_dir_all(missing_dir.join("evidence")).unwrap();
    let missing = serde_json::json!({
        "schema_version": "project-executor-session/v1",
        "execution_id": missing_id,
        "provider": "project-executor",
        "implementation": "jcode-derived-executor/v1",
        "implementation_revision": revision,
        "work_identity": "local:test:missing-legacy",
        "repository": root.path().join("missing-repo"),
        "repository_path": root.path().join("missing-repo"),
        "base_sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "branch": "main",
        "workspace": missing_workspace,
        "state": "READY",
        "created_at": 1,
        "closed_at": null
    });
    std::fs::write(
        missing_dir.join("session.json"),
        serde_json::to_vec_pretty(&missing).unwrap(),
    )
    .unwrap();

    let recovered =
        ProjectExecutorSupervisor::create_with_worktree_root(&sessions, root.path(), revision)
            .unwrap();
    assert_eq!(
        recovered
            .inspect_session(missing_id)
            .await
            .unwrap()
            .runnability,
        SessionRunnability::FailedRecovery
    );
    assert_eq!(
        recovered
            .inspect_session(present_id)
            .await
            .unwrap()
            .runnability,
        SessionRunnability::Quarantined
    );
    let summary = recovered.recovery_summary();
    assert_eq!(summary.runnable, 0);
    assert_eq!(summary.quarantined, 1);
    assert_eq!(summary.failed_recovery, 1);
    assert_eq!(
        summary
            .receipts
            .iter()
            .map(|r| r.session_id.as_str())
            .collect::<Vec<_>>(),
        vec![missing_id, present_id]
    );
    for id in [missing_id, present_id] {
        let error = recovered
            .start_process(id, vec!["true".into()], None, Some(AUTH_DIGEST.to_owned()))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("execution is not runnable"));
    }
}

#[tokio::test]
async fn unsupported_schema_with_safe_identity_is_quarantined_and_sibling_survives() {
    let root = tempfile::tempdir().unwrap();
    let sessions = root.path().join("sessions");
    let revision = "8888888888888888888888888888888888888888";

    let unsupported_id = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    let unsupported_dir = sessions.join(unsupported_id);
    let unsupported_workspace = unsupported_dir.join("workspace");
    let sha = make_repo(&unsupported_workspace);
    std::fs::create_dir_all(unsupported_dir.join("evidence")).unwrap();
    let unsupported = serde_json::json!({
        "schema_version": "project-executor-session/v99",
        "execution_id": unsupported_id,
        "provider": "project-executor",
        "implementation": "jcode-derived-executor/v1",
        "implementation_revision": revision,
        "work_identity": "local:test:unsupported",
        "repository": unsupported_workspace,
        "repository_path": unsupported_workspace,
        "base_sha": sha,
        "branch": "main",
        "workspace": unsupported_workspace,
        "state": "READY",
        "created_at": 1,
        "closed_at": null
    });
    let unsupported_bytes = serde_json::to_vec_pretty(&unsupported).unwrap();
    std::fs::write(unsupported_dir.join("session.json"), &unsupported_bytes).unwrap();

    let malformed_id = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
    let malformed_dir = sessions.join(malformed_id);
    std::fs::create_dir_all(&malformed_dir).unwrap();
    std::fs::write(
        malformed_dir.join("session.json"),
        r#"{"schema_version":"project-executor-session/v99","state":"READY"}"#,
    )
    .unwrap();

    let recovered =
        ProjectExecutorSupervisor::create_with_worktree_root(&sessions, root.path(), revision)
            .unwrap();
    let inspection = recovered.inspect_session(unsupported_id).await.unwrap();
    assert_eq!(inspection.runnability, SessionRunnability::Quarantined);
    assert_eq!(inspection.schema_version, "project-executor-session/v99");
    assert_eq!(
        std::fs::read(unsupported_dir.join("session.json")).unwrap(),
        unsupported_bytes
    );
    assert!(recovered.inspect_session(malformed_id).await.is_err());
    let error = recovered
        .start_process(
            unsupported_id,
            vec!["true".into()],
            None,
            Some(AUTH_DIGEST.to_owned()),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("execution is not runnable"));
    let summary = recovered.recovery_summary();
    assert_eq!(summary.quarantined, 1);
    assert_eq!(summary.failed_recovery, 1);
    assert_eq!(summary.receipts.len(), 1);
    assert_eq!(summary.receipts[0].session_id, unsupported_id);
    assert_eq!(
        summary.receipts[0].reason_codes,
        vec![jcode_project_executor::RecoveryReasonCode::SessionSchemaUnsupported]
    );
}

#[tokio::test]
async fn increment_b_restart_reestablishes_provider_custody_for_write_session() {
    if !Path::new("/usr/bin/bwrap").is_file() {
        return; // Exact behavior is required in the Bookworm/native-bwrap acceptance run.
    }
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo-b-restart");
    let sha = make_repo(&repo);
    let sessions = root.path().join("sessions-b-restart");
    let id = "eeeeeeee-1111-4111-8111-111111111111";
    let revision = "abababababababababababababababababababab";

    let first = ProjectExecutorSupervisor::create(&sessions, revision).unwrap();
    let created = first
        .create_session(request(id, &repo, &sha))
        .await
        .unwrap();
    assert_eq!(created.runnability, SessionRunnability::Runnable);
    assert_eq!(
        created.custody_assessment.as_ref().unwrap().state,
        jcode_project_executor::CustodyState::Confirmed
    );
    let g1 = created
        .custody_assessment
        .as_ref()
        .unwrap()
        .generation
        .clone()
        .unwrap();
    drop(first);

    let recovered = ProjectExecutorSupervisor::create(&sessions, revision).unwrap();
    let inspection = recovered.inspect_session(id).await.unwrap();
    assert_eq!(inspection.runnability, SessionRunnability::Runnable);
    assert_eq!(
        inspection.custody_assessment.as_ref().unwrap().state,
        jcode_project_executor::CustodyState::Confirmed
    );
    let g2 = inspection
        .custody_assessment
        .as_ref()
        .unwrap()
        .generation
        .clone()
        .unwrap();
    assert_ne!(g1, g2);
}

#[tokio::test]
async fn increment_b_write_session_requires_authorization_digest_before_material_setup() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo-auth-required");
    let sha = make_repo(&repo);
    let sessions = root.path().join("sessions-auth-required");
    let id = "cccccccc-dddd-4eee-8fff-aaaaaaaaaaaa";
    let supervisor =
        ProjectExecutorSupervisor::create(&sessions, "acacacacacacacacacacacacacacacacacacacac")
            .unwrap();
    let mut request = request(id, &repo, &sha);
    request.authorization_digest = None;
    let error = supervisor.create_session(request).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("EFFECT_AUTHORIZATION_DIGEST_MISSING")
    );
    assert!(!sessions.join(id).exists());
}

#[tokio::test]
async fn increment_b_pre_authorization_v2_write_record_remains_quarantined() {
    let root = tempfile::tempdir().unwrap();
    let sessions = root.path().join("sessions-pre-auth");
    let id = "dddddddd-eeee-4fff-8aaa-bbbbbbbbbbbb";
    let session_dir = sessions.join(id);
    let workspace = session_dir.join("workspace");
    let sha = make_repo(&workspace);
    std::fs::create_dir_all(session_dir.join("evidence")).unwrap();
    let revision = "adadadadadadadadadadadadadadadadadadadad";
    let historical = serde_json::json!({
        "schema_version": "project-executor-session/v2",
        "execution_id": id,
        "provider": "project-executor",
        "implementation": "jcode-derived-executor/v1",
        "implementation_revision": revision,
        "work_identity": "work:test:pre-auth-v2",
        "repository": workspace,
        "repository_path": workspace,
        "base_sha": sha,
        "branch": "main",
        "workspace": workspace,
        "worktree_binding": {
            "schema_version": "worktree-binding/v1",
            "mode": "EXISTING",
            "ownership": "EXTERNAL",
            "path": workspace,
            "repository": workspace,
            "resolved_sha": sha,
            "access_mode": "write"
        },
        "lifecycle": "READY",
        "runnability": "RUNNABLE",
        "workspace_origin": "EXTERNAL",
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
        serde_json::to_vec_pretty(&historical).unwrap(),
    )
    .unwrap();

    let recovered =
        ProjectExecutorSupervisor::create_with_worktree_root(&sessions, root.path(), revision)
            .unwrap();
    let inspection = recovered.inspect_session(id).await.unwrap();
    assert_eq!(inspection.runnability, SessionRunnability::Quarantined);
    assert_eq!(
        inspection.custody_assessment.as_ref().unwrap().state,
        jcode_project_executor::CustodyState::Unknown
    );
    assert!(inspection.effect_authorization.is_none());
    let error = recovered
        .start_process(id, vec!["true".into()], None, None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("execution is not runnable"));
}
