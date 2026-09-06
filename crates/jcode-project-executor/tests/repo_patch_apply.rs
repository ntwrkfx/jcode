use jcode_project_executor::{ExecutorRequest, ProjectExecutorSupervisor, handle_executor_request};
use serde_json::{Value, json};

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

fn source_repo(root: &std::path::Path) -> (std::path::PathBuf, String) {
    let source = root.join("source");
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
    std::fs::write(source.join("README.md"), "before\n").unwrap();
    git(&source, &["add", "README.md"]);
    git(&source, &["commit", "-q", "-m", "base"]);
    let sha = git(&source, &["rev-parse", "HEAD"]);
    (source, sha)
}

fn patch_request(execution_id: &str, repository: &str, base_sha: &str) -> ExecutorRequest {
    serde_json::from_value(json!({
        "protocol": "project-executor/v1",
        "id": "patch-request-1",
        "command": {
            "op": "repo_patch_apply",
            "execution_id": execution_id,
            "repository": repository,
            "base_sha": base_sha,
            "provider_binding_sha256": format!("sha256:{}", "a".repeat(64)),
            "patch": {
                "schema_version": "repository-patch/v1",
                "format": "unified-diff",
                "content": "--- a/README.md\n+++ b/README.md\n@@ -1 +1 @@\n-before\n+after\n"
            }
        }
    }))
    .unwrap()
}

async fn managed_session() -> (
    tempfile::TempDir,
    ProjectExecutorSupervisor,
    String,
    String,
    String,
) {
    let root = tempfile::tempdir().unwrap();
    let (source, sha) = source_repo(root.path());
    let execution_id = "22222222-2222-4222-8222-222222222222".to_owned();
    let supervisor = ProjectExecutorSupervisor::create_with_worktree_root_and_device_id(
        root.path().join("sessions"),
        root.path(),
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "11111111-1111-4111-8111-111111111111",
    )
    .unwrap();
    let inspection = supervisor
        .create_session(jcode_project_executor::SessionCreateRequest {
            execution_id: execution_id.clone(),
            work_identity: "work:test:repo-patch".to_owned(),
            repository: source.display().to_string(),
            base_sha: sha.clone(),
            worktree_path: None,
            access_mode: None,
        })
        .await
        .unwrap();
    (
        root,
        supervisor,
        execution_id,
        source.display().to_string(),
        inspection.workspace,
    )
}

#[test]
fn protocol_accepts_repo_patch_apply() {
    let root = tempfile::tempdir().unwrap();
    let (source, sha) = source_repo(root.path());
    let parsed = patch_request(
        "22222222-2222-4222-8222-222222222222",
        &source.display().to_string(),
        &sha,
    );
    assert_eq!(parsed.id, "patch-request-1");
}

#[tokio::test]
async fn repo_patch_apply_mutates_only_managed_workspace_and_returns_bound_receipt() {
    let (_root, supervisor, execution_id, repository, workspace) = managed_session().await;
    let base_sha = git(std::path::Path::new(&workspace), &["rev-parse", "HEAD"]);
    let response = handle_executor_request(
        &supervisor,
        patch_request(&execution_id, &repository, &base_sha),
    )
    .await;
    assert!(response.ok, "{:?}", response.error);
    let result: Value = response.result.unwrap();
    assert_eq!(result["schema_version"], "repository-patch-receipt/v1");
    assert_eq!(result["execution_id"], execution_id);
    assert_eq!(result["repository"], repository);
    assert_eq!(result["base_sha"], base_sha);
    assert_eq!(result["patch_sha256"].as_str().unwrap().len(), 71);
    assert_eq!(result["touched_paths"], json!(["README.md"]));
    assert_eq!(result["status"], "APPLIED");
    assert!(
        result["provider_evidence_ref"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert_eq!(
        std::fs::read_to_string(std::path::Path::new(&workspace).join("README.md")).unwrap(),
        "after\n"
    );
    assert_eq!(
        git(std::path::Path::new(&workspace), &["rev-parse", "HEAD"]),
        base_sha
    );
    assert_eq!(
        git(std::path::Path::new(&workspace), &["diff", "--name-only"]),
        "README.md"
    );
}

#[tokio::test]
async fn repo_patch_apply_rejects_repository_drift_and_replay() {
    let (_root, supervisor, execution_id, repository, workspace) = managed_session().await;
    let base_sha = git(std::path::Path::new(&workspace), &["rev-parse", "HEAD"]);
    let mut bad =
        serde_json::to_value(patch_request(&execution_id, &repository, &base_sha)).unwrap();
    bad["command"]["repository"] = json!("wrong/repository");
    let bad: ExecutorRequest = serde_json::from_value(bad).unwrap();
    let rejected = handle_executor_request(&supervisor, bad).await;
    assert!(!rejected.ok);
    assert_eq!(
        std::fs::read_to_string(std::path::Path::new(&workspace).join("README.md")).unwrap(),
        "before\n"
    );

    let first = handle_executor_request(
        &supervisor,
        patch_request(&execution_id, &repository, &base_sha),
    )
    .await;
    assert!(first.ok, "{:?}", first.error);
    let replay = handle_executor_request(
        &supervisor,
        patch_request(&execution_id, &repository, &base_sha),
    )
    .await;
    assert!(
        !replay.ok,
        "replay must fail closed instead of applying twice"
    );
    assert_eq!(
        std::fs::read_to_string(std::path::Path::new(&workspace).join("README.md")).unwrap(),
        "after\n"
    );
}
