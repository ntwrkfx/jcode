use jcode_project_executor::WorktreeBinding;
use serde_json::json;

#[test]
fn worktree_binding_accepts_optional_writer_identity_fields() {
    let binding: WorktreeBinding = serde_json::from_value(json!({
        "schema_version": "worktree-binding/v1",
        "mode": "MANAGED",
        "ownership": "HARNESS",
        "path": "/srv/session/workspace",
        "repository": "/srv/repo",
        "resolved_sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "access_mode": "write",
        "device_id": "11111111-1111-4111-8111-111111111111",
        "git_common_dir": "/srv/repo/.git"
    }))
    .unwrap();
    let encoded = serde_json::to_value(binding).unwrap();
    assert_eq!(encoded["device_id"], "11111111-1111-4111-8111-111111111111");
    assert_eq!(encoded["git_common_dir"], "/srv/repo/.git");
}

#[test]
fn legacy_worktree_binding_remains_valid_without_identity_fields() {
    let binding: WorktreeBinding = serde_json::from_value(json!({
        "schema_version": "worktree-binding/v1",
        "mode": "MANAGED",
        "ownership": "HARNESS",
        "path": "/srv/session/workspace",
        "repository": "/srv/repo",
        "resolved_sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "access_mode": "write"
    }))
    .unwrap();
    let encoded = serde_json::to_value(binding).unwrap();
    assert!(encoded.get("device_id").is_none());
    assert!(encoded.get("git_common_dir").is_none());
}

fn git(repo: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
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

#[tokio::test]
async fn ready_inspection_projects_configured_device_and_workspace_common_dir_without_persisting_them()
 {
    use jcode_project_executor::ProjectExecutorSupervisor;

    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&source).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .arg(&source)
            .status()
            .unwrap()
            .success()
    );
    git(&source, &["config", "user.name", "Executor Test"]);
    git(
        &source,
        &["config", "user.email", "executor@example.invalid"],
    );
    std::fs::write(source.join("README.md"), "first\n").unwrap();
    git(&source, &["add", "README.md"]);
    git(&source, &["commit", "-q", "-m", "first"]);
    let sha = git(&source, &["rev-parse", "HEAD"]);
    assert!(
        std::process::Command::new("git")
            .args(["clone", "-q"])
            .arg(&source)
            .arg(&workspace)
            .status()
            .unwrap()
            .success()
    );

    let execution_id = "22222222-2222-4222-8222-222222222222";
    let sessions = root.path().join("sessions");
    let session_dir = sessions.join(execution_id);
    std::fs::create_dir_all(&session_dir).unwrap();
    let record = json!({
        "schema_version": "project-executor-session/v1",
        "execution_id": execution_id,
        "provider": "project-executor",
        "implementation": "jcode-derived-executor/v1",
        "implementation_revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "work_identity": "local:test:writer-identity",
        "repository": source,
        "repository_path": source,
        "base_sha": sha,
        "branch": "main",
        "workspace": workspace,
        "worktree_binding": {
            "schema_version": "worktree-binding/v1", "mode": "MANAGED",
            "ownership": "HARNESS", "path": workspace, "repository": source,
            "resolved_sha": sha, "access_mode": "write"
        },
        "state": "READY", "created_at": 1, "closed_at": null
    });
    std::fs::write(
        session_dir.join("session.json"),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();

    let device_id = "11111111-1111-4111-8111-111111111111";
    let supervisor = ProjectExecutorSupervisor::create_with_worktree_root_and_device_id(
        &sessions,
        root.path(),
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        device_id,
    )
    .unwrap();
    let inspected = supervisor.inspect_session(execution_id).await.unwrap();
    assert_eq!(
        inspected.worktree_binding.device_id.as_deref(),
        Some(device_id)
    );
    let expected_common = std::fs::canonicalize(workspace.join(".git")).unwrap();
    assert_eq!(
        inspected.worktree_binding.git_common_dir.as_deref(),
        expected_common.to_str()
    );
    assert_ne!(
        expected_common,
        std::fs::canonicalize(source.join(".git")).unwrap()
    );

    let persisted = std::fs::read_to_string(session_dir.join("session.json")).unwrap();
    assert!(!persisted.contains("device_id"));
    assert!(!persisted.contains("git_common_dir"));
}
