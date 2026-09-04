use jcode_project_executor::custody::{CustodyAcquireRequest, CustodyScope, LocalCustodyProvider};
use jcode_project_executor::effect::EffectAuthorizationBinding;
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
    let workspace_identity = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf())
        .display()
        .to_string();
    RetirementIntent {
        retirement_intent_id: id.to_owned(),
        work_id: "work:test:c-retirement-api-red".to_owned(),
        execution_id: execution_id.to_owned(),
        resource_identity: RESOURCE.to_owned(),
        workspace_identity: workspace_identity.clone(),
        candidate_revision: sha.to_owned(),
        effect_class: WORKSPACE_RETIRE_EFFECT_CLASS.to_owned(),
        authorization_digest: AUTH_DIGEST.to_owned(),
        authorization_binding: Some(EffectAuthorizationBinding {
            work_id: "work:test:c-retirement-api-red".to_owned(),
            execution_id: execution_id.to_owned(),
            resource_identity: RESOURCE.to_owned(),
            workspace_identity,
            candidate_revision: sha.to_owned(),
            effect_class: WORKSPACE_RETIRE_EFFECT_CLASS.to_owned(),
            authorization_digest: AUTH_DIGEST.to_owned(),
        }),
        expected_material: jcode_project_executor::ExpectedMaterial {
            head_sha: sha.to_owned(),
            local_material: jcode_project_executor::LocalMaterialState::None,
        },
        disposition: RetirementDisposition::DiscardCleanManagedWorkspace,
    }
}

fn set_historical_custody(
    sessions: &Path,
    execution_id: &str,
    executor: &str,
    provider: &str,
    token: &str,
) {
    let path = sessions.join(execution_id).join("session.json");
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    record["custody_assessment"] = serde_json::json!({
        "state": "CONFIRMED",
        "assessed_at": 1,
        "executor_instance_id": executor,
        "generation": {"provider": provider, "token": token}
    });
    std::fs::write(path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
}

fn hold_current_scope(
    sessions: &Path,
    execution_id: &str,
    workspace: &Path,
    executor: &str,
) -> (
    LocalCustodyProvider,
    jcode_project_executor::custody::CustodyGrant,
) {
    let mut provider =
        LocalCustodyProvider::new("C_RETIREMENT_ADVERSARIAL", sessions.join(".worktree-locks"))
            .unwrap();
    let grant = provider
        .acquire(CustodyAcquireRequest {
            scope: CustodyScope {
                execution_id: execution_id.to_owned(),
                collision_identity: workspace.canonicalize().unwrap().display().to_string(),
            },
            executor_instance_id: executor.to_owned(),
        })
        .unwrap();
    (provider, grant)
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
async fn syntactically_valid_digest_without_independently_admitted_binding_denies_before_material_effect()
 {
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
    request.authorization_binding = None;

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

#[tokio::test]
async fn current_custody_held_by_wrong_executor_denies_retirement() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo-custody-blocked");
    let sha = make_repo(&repo);
    let workspace = root.path().join("managed-custody-blocked");
    add_managed_worktree(&repo, &workspace, &sha);
    let sessions = root.path().join("sessions-custody-blocked");
    let execution_id = "c3000000-0000-4000-8000-000000000003";
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
    let (_blocker, _grant) =
        hold_current_scope(&sessions, execution_id, &workspace, "wrong-executor");

    let receipt = supervisor
        .retire_workspace(intent(
            "retire-custody-blocked",
            execution_id,
            &workspace,
            &sha,
        ))
        .await
        .unwrap();
    assert_eq!(receipt.outcome, RetirementOutcome::Denied);
    assert_eq!(
        receipt.reason,
        Some(RetirementReasonCode::CustodyNotCurrent)
    );
    assert!(workspace.is_dir());
}

#[tokio::test]
async fn persisted_stale_g1_cannot_replace_fresh_retirement_custody() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo-stale-g1");
    let sha = make_repo(&repo);
    let workspace = root.path().join("managed-stale-g1");
    add_managed_worktree(&repo, &workspace, &sha);
    let sessions = root.path().join("sessions-stale-g1");
    let execution_id = "c4000000-0000-4000-8000-000000000004";
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
    set_historical_custody(
        &sessions,
        execution_id,
        "old-executor",
        "OLD_PROVIDER",
        "G1",
    );
    let supervisor = supervisor_for(&sessions, root.path()).await;
    let (_current, _g2) = hold_current_scope(
        &sessions,
        execution_id,
        &workspace,
        "current-other-executor",
    );

    let receipt = supervisor
        .retire_workspace(intent("retire-stale-g1", execution_id, &workspace, &sha))
        .await
        .unwrap();
    assert_eq!(receipt.outcome, RetirementOutcome::Denied);
    assert_eq!(
        receipt.reason,
        Some(RetirementReasonCode::CustodyNotCurrent)
    );
    assert!(workspace.is_dir());
}

#[tokio::test]
async fn clean_retirement_reacquires_generation_distinct_from_historical_g1() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo-fresh-g2");
    let sha = make_repo(&repo);
    let workspace = root.path().join("managed-fresh-g2");
    add_managed_worktree(&repo, &workspace, &sha);
    let sessions = root.path().join("sessions-fresh-g2");
    let execution_id = "c5000000-0000-4000-8000-000000000005";
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
    set_historical_custody(
        &sessions,
        execution_id,
        "old-executor",
        "OLD_PROVIDER",
        "G1",
    );
    let supervisor = supervisor_for(&sessions, root.path()).await;

    let receipt = supervisor
        .retire_workspace(intent("retire-fresh-g2", execution_id, &workspace, &sha))
        .await
        .unwrap();
    assert_eq!(receipt.outcome, RetirementOutcome::Retired);
    let generation = receipt
        .custody_generation
        .expect("retirement receipt must bind fresh B generation");
    assert_ne!(generation.token, "G1");
}

#[tokio::test]
async fn material_unknown_denies_and_preserves_workspace() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo-unknown");
    let sha = make_repo(&repo);
    let workspace = root.path().join("managed-unknown");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("opaque.bin"), b"unknown material").unwrap();
    let sessions = root.path().join("sessions-unknown");
    let execution_id = "c6000000-0000-4000-8000-000000000006";
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
        .retire_workspace(intent("retire-unknown", execution_id, &workspace, &sha))
        .await
        .unwrap();
    assert_eq!(receipt.outcome, RetirementOutcome::Denied);
    assert_eq!(receipt.reason, Some(RetirementReasonCode::MaterialUnknown));
    assert!(workspace.join("opaque.bin").is_file());
}

struct GitWrapperGuard {
    old_path: std::ffi::OsString,
    old_mode: Option<std::ffi::OsString>,
    old_target: Option<std::ffi::OsString>,
    old_counter: Option<std::ffi::OsString>,
}

impl Drop for GitWrapperGuard {
    fn drop(&mut self) {
        unsafe {
            std::env::set_var("PATH", &self.old_path);
            match &self.old_mode {
                Some(v) => std::env::set_var("C_RED_GIT_MODE", v),
                None => std::env::remove_var("C_RED_GIT_MODE"),
            }
            match &self.old_target {
                Some(v) => std::env::set_var("C_RED_GIT_TARGET", v),
                None => std::env::remove_var("C_RED_GIT_TARGET"),
            }
            match &self.old_counter {
                Some(v) => std::env::set_var("C_RED_GIT_COUNTER", v),
                None => std::env::remove_var("C_RED_GIT_COUNTER"),
            }
        }
    }
}

#[cfg(unix)]
fn install_git_wrapper(root: &Path, target: &Path, mode: &str) -> GitWrapperGuard {
    use std::os::unix::fs::PermissionsExt;
    let wrapper_root = root.join("git-wrapper");
    std::fs::create_dir_all(&wrapper_root).unwrap();
    let wrapper = wrapper_root.join("git");
    let counter = wrapper_root.join("counter");
    std::fs::write(&counter, "0\n").unwrap();
    std::fs::write(
        &wrapper,
        r#"#!/bin/sh
REAL=/usr/bin/git
TARGET="$C_RED_GIT_TARGET"
MODE="$C_RED_GIT_MODE"
COUNTER="$C_RED_GIT_COUNTER"
ARGS=" $* "
case "$ARGS" in
  *"$TARGET"*)
    if [ "$MODE" = "toctou" ]; then
      case "$ARGS" in
        *" status "*)
          n=$(cat "$COUNTER" 2>/dev/null || echo 0)
          n=$((n + 1))
          printf '%s\n' "$n" > "$COUNTER"
          if [ "$n" -eq 2 ]; then printf 'changed\n' > "$TARGET/toctou.txt"; fi
          ;;
      esac
    fi
    if [ "$MODE" = "effect_fail" ]; then
      case "$ARGS" in *" worktree remove "*) exit 97;; esac
    fi
    if [ "$MODE" = "ambiguous_after_remove" ]; then
      case "$ARGS" in
        *" worktree remove "*)
          "$REAL" "$@"
          rc=$?
          if [ "$rc" -eq 0 ]; then exit 98; fi
          exit "$rc"
          ;;
      esac
    fi
    ;;
esac
exec "$REAL" "$@"
"#,
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&wrapper).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&wrapper, permissions).unwrap();

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = format!("{}:{}", wrapper_root.display(), old_path.to_string_lossy());
    let guard = GitWrapperGuard {
        old_path,
        old_mode: std::env::var_os("C_RED_GIT_MODE"),
        old_target: std::env::var_os("C_RED_GIT_TARGET"),
        old_counter: std::env::var_os("C_RED_GIT_COUNTER"),
    };
    unsafe {
        std::env::set_var("PATH", new_path);
        std::env::set_var("C_RED_GIT_MODE", mode);
        std::env::set_var("C_RED_GIT_TARGET", target.canonicalize().unwrap());
        std::env::set_var("C_RED_GIT_COUNTER", counter);
    }
    guard
}

#[cfg(unix)]
#[tokio::test]
async fn clean_preflight_then_material_change_before_effect_is_denied() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo-toctou");
    let sha = make_repo(&repo);
    let workspace = root.path().join("managed-toctou");
    add_managed_worktree(&repo, &workspace, &sha);
    let sessions = root.path().join("sessions-toctou");
    let execution_id = "c7000000-0000-4000-8000-000000000007";
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
    let guard = install_git_wrapper(root.path(), &workspace, "toctou");

    let receipt = supervisor
        .retire_workspace(intent("retire-toctou", execution_id, &workspace, &sha))
        .await
        .unwrap();
    drop(guard);
    assert_eq!(receipt.outcome, RetirementOutcome::Denied);
    assert_eq!(receipt.reason, Some(RetirementReasonCode::MaterialChanged));
    assert!(workspace.join("toctou.txt").is_file());
}

#[cfg(unix)]
#[tokio::test]
async fn worktree_remove_failure_preserves_closed_material_and_never_reports_retired() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo-effect-fail");
    let sha = make_repo(&repo);
    let workspace = root.path().join("managed-effect-fail");
    add_managed_worktree(&repo, &workspace, &sha);
    let sessions = root.path().join("sessions-effect-fail");
    let execution_id = "c8000000-0000-4000-8000-000000000008";
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
    let guard = install_git_wrapper(root.path(), &workspace, "effect_fail");

    let receipt = supervisor
        .retire_workspace(intent("retire-effect-fail", execution_id, &workspace, &sha))
        .await
        .unwrap();
    drop(guard);
    assert_eq!(receipt.outcome, RetirementOutcome::Failed);
    assert_eq!(receipt.reason, Some(RetirementReasonCode::EffectFailed));
    assert!(workspace.is_dir());
    let disk: serde_json::Value = serde_json::from_slice(
        &std::fs::read(sessions.join(execution_id).join("session.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(disk["lifecycle"], "CLOSED");
}

#[cfg(unix)]
#[tokio::test]
async fn delete_effect_succeeds_but_terminal_result_is_lost_remains_ambiguous() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo-ambiguous");
    let sha = make_repo(&repo);
    let workspace = root.path().join("managed-ambiguous");
    add_managed_worktree(&repo, &workspace, &sha);
    let sessions = root.path().join("sessions-ambiguous");
    let execution_id = "c9000000-0000-4000-8000-000000000009";
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
    let guard = install_git_wrapper(root.path(), &workspace, "ambiguous_after_remove");

    let receipt = supervisor
        .retire_workspace(intent("retire-ambiguous", execution_id, &workspace, &sha))
        .await
        .unwrap();
    drop(guard);
    assert_eq!(receipt.outcome, RetirementOutcome::OutcomeAmbiguous);
    assert_eq!(receipt.reason, Some(RetirementReasonCode::OutcomeAmbiguous));
    assert_ne!(receipt.outcome, RetirementOutcome::Retired);
    assert!(
        !workspace.exists(),
        "test must prove the destructive effect actually occurred"
    );
}

#[tokio::test]
async fn current_generation_for_wrong_collision_cannot_authorize_target_retirement() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo-wrong-collision");
    let sha = make_repo(&repo);
    let target = root.path().join("managed-target-collision");
    let unrelated = root.path().join("managed-unrelated-collision");
    add_managed_worktree(&repo, &target, &sha);
    add_managed_worktree(&repo, &unrelated, &sha);
    let sessions = root.path().join("sessions-wrong-collision");
    let execution_id = "ca000000-0000-4000-8000-00000000000a";
    write_closed_record(
        &sessions,
        execution_id,
        &repo,
        &target,
        &sha,
        "MANAGED",
        "MANAGED",
        "HARNESS",
    );
    let mut provider = LocalCustodyProvider::new(
        "C_RETIREMENT_WRONG_COLLISION",
        sessions.join(".worktree-locks"),
    )
    .unwrap();
    let unrelated_grant = provider
        .acquire(CustodyAcquireRequest {
            scope: CustodyScope {
                execution_id: execution_id.to_owned(),
                collision_identity: unrelated.canonicalize().unwrap().display().to_string(),
            },
            executor_instance_id: "wrong-collision-executor".to_owned(),
        })
        .unwrap();
    set_historical_custody(
        &sessions,
        execution_id,
        &unrelated_grant.executor_instance_id,
        &unrelated_grant.generation.provider,
        &unrelated_grant.generation.token,
    );
    let supervisor = supervisor_for(&sessions, root.path()).await;

    let receipt = supervisor
        .retire_workspace(intent(
            "retire-wrong-collision",
            execution_id,
            &target,
            &sha,
        ))
        .await
        .unwrap();
    assert_eq!(receipt.outcome, RetirementOutcome::Retired);
    let fresh = receipt
        .custody_generation
        .expect("target retirement needs an exact-scope fresh generation");
    assert_ne!(fresh.token, unrelated_grant.generation.token);
    assert!(unrelated.is_dir());
}
