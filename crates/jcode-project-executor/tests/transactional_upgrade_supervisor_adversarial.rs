use jcode_project_executor::retirement::{RetirementDisposition, RetirementIntent};
use jcode_project_executor::{
    ExpectedMaterial, LocalMaterialState, ProjectExecutorSupervisor, ResumeIntent,
    SessionCreateRequest, UpgradePhase, event_digest,
};
use std::path::Path;
use std::process::Command;

const AUTH_DIGEST: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
const REV1: &str = "1111111111111111111111111111111111111111";
const REV2: &str = "2222222222222222222222222222222222222222";
const REV3: &str = "3333333333333333333333333333333333333333";

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
    git(path, &["config", "user.name", "D Test"]);
    git(path, &["config", "user.email", "d@example.invalid"]);
    std::fs::write(path.join("README.md"), "d\n").unwrap();
    git(path, &["add", "README.md"]);
    git(path, &["commit", "-q", "-m", "d"]);
    git(path, &["rev-parse", "HEAD"])
}

fn request(id: &str, repo: &Path, sha: &str) -> SessionCreateRequest {
    SessionCreateRequest {
        execution_id: id.into(),
        work_identity: format!("work:{id}"),
        repository: repo.display().to_string(),
        base_sha: sha.into(),
        worktree_path: None,
        access_mode: None,
        authorization_digest: Some(AUTH_DIGEST.into()),
    }
}

fn resume_intent(
    state: &jcode_project_executor::TransactionalUpgradeState,
    event: &str,
    payload: &str,
) -> ResumeIntent {
    ResumeIntent::new(
        state.request.work_id.clone(),
        state.request.attempt_id.clone(),
        state.request.execution_id.clone(),
        state.request.checkpoint_generation,
        event,
        event_digest(payload),
        state.successor_generation().to_owned(),
        state.successor_executor_instance_id.clone().unwrap(),
        state.request.expected_successor_runtime_identity.clone(),
        state.request.expected_material_identity.clone(),
    )
    .unwrap()
}

#[tokio::test]
async fn prepare_quiesces_releases_g1_and_blocks_old_effects() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let sessions = root.path().join("sessions");
    let id = "11111111-2222-4333-8444-555555555555";
    let supervisor = ProjectExecutorSupervisor::create(&sessions, REV1).unwrap();
    let created = supervisor
        .create_session(request(id, &repo, &sha))
        .await
        .unwrap();
    let g1 = created
        .custody_assessment
        .unwrap()
        .generation
        .unwrap()
        .token;
    let state = supervisor
        .prepare_transactional_upgrade(id, "attempt-1", REV2, 7)
        .await
        .unwrap();
    assert_eq!(state.phase, UpgradePhase::StartSuccessor);
    assert!(!state.predecessor_authority_current);
    assert_eq!(state.predecessor_generation(), g1);
    assert!(!state.admissions_open);
    let err = supervisor
        .start_process(id, vec!["true".into()], None, Some(AUTH_DIGEST.into()))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("D_ADMISSIONS_QUIESCED"));
    assert!(
        supervisor
            .create_session(request("61111111-2222-4333-8444-555555555555", &repo, &sha))
            .await
            .unwrap_err()
            .to_string()
            .contains("D_ADMISSIONS_QUIESCED")
    );
    assert!(sessions.join(".transactional-upgrade/state.json").is_file());
}

#[tokio::test]
async fn successor_recovery_uses_fresh_generation_and_requires_resume_before_effect() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let sessions = root.path().join("sessions");
    let id = "21111111-2222-4333-8444-555555555555";
    let first = ProjectExecutorSupervisor::create(&sessions, REV1).unwrap();
    let created = first
        .create_session(request(id, &repo, &sha))
        .await
        .unwrap();
    let g1 = created
        .custody_assessment
        .unwrap()
        .generation
        .unwrap()
        .token;
    first
        .prepare_transactional_upgrade(id, "attempt-1", REV2, 7)
        .await
        .unwrap();
    drop(first);
    let second = ProjectExecutorSupervisor::create(&sessions, REV2).unwrap();
    let state = second.inspect_transactional_upgrade().unwrap();
    assert_eq!(state.phase, UpgradePhase::ResumeAdmission);
    assert_ne!(state.successor_generation(), g1);
    assert!(!state.admissions_open);
    assert!(
        second
            .start_process(id, vec!["true".into()], None, Some(AUTH_DIGEST.into()))
            .await
            .unwrap_err()
            .to_string()
            .contains("D_ADMISSIONS_QUIESCED")
    );
    let payload = "payload-event-1";
    let intent = resume_intent(&state, "event-1", payload);
    let complete = second
        .resume_transactional_upgrade(intent.clone(), payload)
        .unwrap();
    assert_eq!(complete.phase, UpgradePhase::Complete);
    assert!(complete.admissions_open);
    assert!(
        second
            .resume_transactional_upgrade(intent, payload)
            .is_err()
    );
    let p = second
        .start_process(id, vec!["true".into()], None, Some(AUTH_DIGEST.into()))
        .await
        .unwrap();
    second.wait_process(id, &p.process_id, None).await.unwrap();
}

#[tokio::test]
async fn caller_supplied_digest_cannot_replace_submitted_event_binding() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let sessions = root.path().join("sessions");
    let id = "71111111-2222-4333-8444-555555555555";
    let first = ProjectExecutorSupervisor::create(&sessions, REV1).unwrap();
    first
        .create_session(request(id, &repo, &sha))
        .await
        .unwrap();
    first
        .prepare_transactional_upgrade(id, "attempt-1", REV2, 7)
        .await
        .unwrap();
    drop(first);
    let second = ProjectExecutorSupervisor::create(&sessions, REV2).unwrap();
    let state = second.inspect_transactional_upgrade().unwrap();
    let intent = resume_intent(&state, "event-1", "expected-payload");
    let error = second
        .resume_transactional_upgrade(intent, "different-payload")
        .unwrap_err();
    assert!(error.to_string().contains("D_RESUME_EVENT_DIGEST_MISMATCH"));
    assert!(
        !second
            .inspect_transactional_upgrade()
            .unwrap()
            .admissions_open
    );
}

#[tokio::test]
async fn stale_g2_resume_is_rejected_after_successor_restarts_again() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let sessions = root.path().join("sessions");
    let id = "31111111-2222-4333-8444-555555555555";
    let first = ProjectExecutorSupervisor::create(&sessions, REV1).unwrap();
    first
        .create_session(request(id, &repo, &sha))
        .await
        .unwrap();
    first
        .prepare_transactional_upgrade(id, "attempt-1", REV2, 7)
        .await
        .unwrap();
    drop(first);
    let second = ProjectExecutorSupervisor::create(&sessions, REV2).unwrap();
    let state2 = second.inspect_transactional_upgrade().unwrap();
    let payload = "payload-event-1";
    let stale = resume_intent(&state2, "event-1", payload);
    let g2 = state2.successor_generation().to_owned();
    drop(second);
    let third = ProjectExecutorSupervisor::create(&sessions, REV2).unwrap();
    let state3 = third.inspect_transactional_upgrade().unwrap();
    assert_ne!(state3.successor_generation(), g2);
    assert!(
        third
            .resume_transactional_upgrade(stale, payload)
            .unwrap_err()
            .to_string()
            .contains("D_RESUME_GENERATION_STALE")
    );
}

#[tokio::test]
async fn concurrent_resume_accepts_at_most_one_transition() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let sessions = root.path().join("sessions");
    let id = "81111111-2222-4333-8444-555555555555";
    let first = ProjectExecutorSupervisor::create(&sessions, REV1).unwrap();
    first
        .create_session(request(id, &repo, &sha))
        .await
        .unwrap();
    first
        .prepare_transactional_upgrade(id, "attempt-1", REV2, 7)
        .await
        .unwrap();
    drop(first);
    let second = std::sync::Arc::new(ProjectExecutorSupervisor::create(&sessions, REV2).unwrap());
    let state = second.inspect_transactional_upgrade().unwrap();
    let payload = "concurrent payload";
    let intent = resume_intent(&state, "event-concurrent", payload);
    let mut joins = Vec::new();
    for _ in 0..2 {
        let supervisor = second.clone();
        let intent = intent.clone();
        joins.push(std::thread::spawn(move || {
            supervisor
                .resume_transactional_upgrade(intent, payload)
                .is_ok()
        }));
    }
    let accepted = joins
        .into_iter()
        .map(|join| join.join().unwrap())
        .filter(|accepted| *accepted)
        .count();
    assert_eq!(accepted, 1);
    let final_state = second.inspect_transactional_upgrade().unwrap();
    assert_eq!(final_state.phase, UpgradePhase::Complete);
    assert_eq!(final_state.continuation_count, 1);
}

#[tokio::test]
async fn wrong_successor_revision_fails_closed_and_does_not_reopen_admission() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let sessions = root.path().join("sessions");
    let id = "41111111-2222-4333-8444-555555555555";
    let first = ProjectExecutorSupervisor::create(&sessions, REV1).unwrap();
    first
        .create_session(request(id, &repo, &sha))
        .await
        .unwrap();
    first
        .prepare_transactional_upgrade(id, "attempt-1", REV2, 7)
        .await
        .unwrap();
    drop(first);
    let wrong =
        ProjectExecutorSupervisor::create(&sessions, "3333333333333333333333333333333333333333")
            .unwrap();
    let state = wrong.inspect_transactional_upgrade().unwrap();
    assert_eq!(state.phase, UpgradePhase::Failed);
    assert!(!state.admissions_open);
    assert!(
        wrong
            .create_session(request("51111111-2222-4333-8444-555555555555", &repo, &sha))
            .await
            .unwrap_err()
            .to_string()
            .contains("D_ADMISSIONS_QUIESCED")
    );
}

#[cfg(unix)]
static PATH_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(unix)]
struct PathGuard(std::ffi::OsString);
#[cfg(unix)]
impl Drop for PathGuard {
    fn drop(&mut self) {
        unsafe { std::env::set_var("PATH", &self.0) };
    }
}

#[cfg(unix)]
fn install_blocking_worktree_add_git(
    root: &Path,
) -> (PathGuard, std::path::PathBuf, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let bin = root.join("git-wrapper-quiesce");
    std::fs::create_dir_all(&bin).unwrap();
    let entered = root.join("worktree-add-entered");
    let release = root.join("worktree-add-release");
    let wrapper = bin.join("git");
    std::fs::write(
        &wrapper,
        format!(
            r#"#!/bin/sh
case " $* " in
  *"{root}"*" worktree add --detach "*)
    : > '{entered}'
    while [ ! -e '{release}' ]; do sleep 0.01; done
    ;;
esac
exec /usr/bin/git "$@"
"#,
            root = root.display(),
            entered = entered.display(),
            release = release.display()
        ),
    )
    .unwrap();
    let mut mode = std::fs::metadata(&wrapper).unwrap().permissions();
    mode.set_mode(0o755);
    std::fs::set_permissions(&wrapper, mode).unwrap();
    let old = std::env::var_os("PATH").unwrap_or_default();
    unsafe {
        std::env::set_var(
            "PATH",
            format!("{}:{}", bin.display(), old.to_string_lossy()),
        )
    };
    (PathGuard(old), entered, release)
}

#[cfg(unix)]
fn install_second_status_dirty_git(root: &Path) -> PathGuard {
    use std::os::unix::fs::PermissionsExt;
    let bin = root.join("git-wrapper-d");
    std::fs::create_dir_all(&bin).unwrap();
    let counter = root.join("git-status-count");
    std::fs::write(&counter, "0\n").unwrap();
    let wrapper = bin.join("git");
    std::fs::write(
        &wrapper,
        format!(
            r#"#!/bin/sh
case " $* " in
  *"{root}"*" status --porcelain"*)
    n=$(cat '{counter}')
    n=$((n + 1))
    printf '%s\n' "$n" > '{counter}'
    if [ "$n" -eq 2 ]; then
      printf ' M README.md\n'
      exit 0
    fi
    ;;
esac
exec /usr/bin/git "$@"
"#,
            root = root.display(),
            counter = counter.display()
        ),
    )
    .unwrap();
    let mut mode = std::fs::metadata(&wrapper).unwrap().permissions();
    mode.set_mode(0o755);
    std::fs::set_permissions(&wrapper, mode).unwrap();
    let old = std::env::var_os("PATH").unwrap_or_default();
    unsafe {
        std::env::set_var(
            "PATH",
            format!("{}:{}", bin.display(), old.to_string_lossy()),
        )
    };
    PathGuard(old)
}

#[cfg(unix)]
#[tokio::test]
async fn successor_verification_reobserves_material_after_recovery_and_g2() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let sessions = root.path().join("sessions");
    let id = "91111111-2222-4333-8444-555555555555";
    let first = ProjectExecutorSupervisor::create(&sessions, REV1).unwrap();
    first
        .create_session(request(id, &repo, &sha))
        .await
        .unwrap();
    first
        .prepare_transactional_upgrade(id, "attempt-1", REV2, 7)
        .await
        .unwrap();
    drop(first);

    let _path_env_lock = PATH_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _path = install_second_status_dirty_git(root.path());
    let successor = ProjectExecutorSupervisor::create(&sessions, REV2).unwrap();
    let state = successor.inspect_transactional_upgrade().unwrap();
    assert_eq!(state.phase, UpgradePhase::Failed);
    assert!(!state.admissions_open);
    assert!(!state.material_verified);
    assert!(
        state
            .failure_reason
            .as_deref()
            .unwrap_or("")
            .contains("D_SUCCESSOR_VERIFICATION_FAILED_OR_UNKNOWN")
    );
}

#[tokio::test]
async fn completed_upgrade_can_begin_a_second_replacement_with_fresh_predecessor_generation() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let sessions = root.path().join("sessions");
    let id = "a1111111-2222-4333-8444-555555555555";
    let first = ProjectExecutorSupervisor::create(&sessions, REV1).unwrap();
    first
        .create_session(request(id, &repo, &sha))
        .await
        .unwrap();
    first
        .prepare_transactional_upgrade(id, "attempt-1", REV2, 7)
        .await
        .unwrap();
    drop(first);

    let second = ProjectExecutorSupervisor::create(&sessions, REV2).unwrap();
    let state = second.inspect_transactional_upgrade().unwrap();
    let g2 = state.successor_generation().to_owned();
    let payload = "first replacement payload";
    let intent = resume_intent(&state, "event-first-replacement", payload);
    let complete = second
        .resume_transactional_upgrade(intent, payload)
        .unwrap();
    assert_eq!(complete.phase, UpgradePhase::Complete);
    assert_eq!(complete.request.checkpoint_generation, 8);

    let next = second
        .prepare_transactional_upgrade(id, "attempt-2", REV3, 8)
        .await
        .unwrap();
    assert_eq!(next.phase, UpgradePhase::StartSuccessor);
    assert_eq!(next.predecessor_generation(), g2);
    assert!(!next.admissions_open);
    assert!(sessions.join(".transactional-upgrade/history").is_dir());
}

#[tokio::test]
async fn consumed_event_id_remains_consumed_across_completed_transaction_rotation() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let sessions = root.path().join("sessions");
    let id = "b1111111-2222-4333-8444-555555555555";
    let first = ProjectExecutorSupervisor::create(&sessions, REV1).unwrap();
    first
        .create_session(request(id, &repo, &sha))
        .await
        .unwrap();
    first
        .prepare_transactional_upgrade(id, "attempt-1", REV2, 7)
        .await
        .unwrap();
    drop(first);

    let second = ProjectExecutorSupervisor::create(&sessions, REV2).unwrap();
    let first_state = second.inspect_transactional_upgrade().unwrap();
    let payload = "globally consumed payload";
    let first_intent = resume_intent(&first_state, "event-global-1", payload);
    second
        .resume_transactional_upgrade(first_intent, payload)
        .unwrap();
    second
        .prepare_transactional_upgrade(id, "attempt-2", REV3, 8)
        .await
        .unwrap();
    drop(second);

    let third = ProjectExecutorSupervisor::create(&sessions, REV3).unwrap();
    let second_state = third.inspect_transactional_upgrade().unwrap();
    assert_eq!(second_state.phase, UpgradePhase::ResumeAdmission);
    let replay = resume_intent(&second_state, "event-global-1", payload);
    let error = third
        .resume_transactional_upgrade(replay, payload)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("D_RESUME_EVENT_ALREADY_CONSUMED")
    );
    assert!(
        !third
            .inspect_transactional_upgrade()
            .unwrap()
            .admissions_open
    );
}

#[tokio::test]
async fn logical_close_is_blocked_while_replacement_is_quiesced() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let sessions = root.path().join("sessions");
    let id = "c1111111-2222-4333-8444-555555555555";
    let supervisor = ProjectExecutorSupervisor::create(&sessions, REV1).unwrap();
    supervisor
        .create_session(request(id, &repo, &sha))
        .await
        .unwrap();
    supervisor
        .prepare_transactional_upgrade(id, "attempt-1", REV2, 7)
        .await
        .unwrap();
    let error = supervisor.close_session(id).await.unwrap_err();
    assert!(error.to_string().contains("D_ADMISSIONS_QUIESCED"));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quiesce_waits_for_inflight_admission_before_replacement_advances() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let sessions = root.path().join("sessions");
    let target = "d1111111-2222-4333-8444-555555555555";
    let other = "e1111111-2222-4333-8444-555555555555";
    let supervisor =
        std::sync::Arc::new(ProjectExecutorSupervisor::create(&sessions, REV1).unwrap());
    supervisor
        .create_session(request(target, &repo, &sha))
        .await
        .unwrap();
    let _path_env_lock = PATH_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (_path, entered, release) = install_blocking_worktree_add_git(root.path());

    let create_supervisor = supervisor.clone();
    let create_request = request(other, &repo, &sha);
    let create =
        tokio::spawn(async move { create_supervisor.create_session(create_request).await });
    for _ in 0..200 {
        if entered.is_file() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        entered.is_file(),
        "blocked admission never reached git worktree add"
    );

    let prepare_supervisor = supervisor.clone();
    let prepare = tokio::spawn(async move {
        prepare_supervisor
            .prepare_transactional_upgrade(target, "attempt-1", REV2, 7)
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let advanced_early = prepare.is_finished();

    std::fs::write(&release, b"release\n").unwrap();
    create.await.unwrap().unwrap();
    let state = prepare.await.unwrap().unwrap();
    assert!(
        !advanced_early,
        "QUIESCE advanced past an in-flight admission"
    );
    assert_eq!(state.phase, UpgradePhase::StartSuccessor);
    assert!(!state.admissions_open);
}

#[tokio::test]
async fn retirement_is_blocked_while_replacement_is_quiesced() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let sessions = root.path().join("sessions");
    let id = "f1111111-2222-4333-8444-555555555555";
    let supervisor = ProjectExecutorSupervisor::create(&sessions, REV1).unwrap();
    supervisor
        .create_session(request(id, &repo, &sha))
        .await
        .unwrap();
    supervisor
        .prepare_transactional_upgrade(id, "attempt-1", REV2, 7)
        .await
        .unwrap();

    let intent = RetirementIntent {
        retirement_intent_id: "d-quiesced-retirement".into(),
        work_id: format!("work:{id}"),
        execution_id: id.into(),
        resource_identity: repo.display().to_string(),
        workspace_identity: sessions.join(id).join("workspace").display().to_string(),
        candidate_revision: sha.clone(),
        effect_class: "WORKSPACE_RETIRE".into(),
        authorization_digest: AUTH_DIGEST.into(),
        authorization_binding: None,
        expected_material: ExpectedMaterial {
            head_sha: sha,
            local_material: LocalMaterialState::None,
        },
        disposition: RetirementDisposition::DiscardCleanManagedWorkspace,
    };
    let error = supervisor.retire_workspace(intent).await.unwrap_err();
    assert!(error.to_string().contains("D_ADMISSIONS_QUIESCED"));
}

#[tokio::test]
async fn successor_rejects_corrupted_durable_checkpoint_instead_of_echoing_request_generation() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let sessions = root.path().join("sessions");
    let id = "f2111111-2222-4333-8444-555555555555";
    let first = ProjectExecutorSupervisor::create(&sessions, REV1).unwrap();
    first
        .create_session(request(id, &repo, &sha))
        .await
        .unwrap();
    first
        .prepare_transactional_upgrade(id, "attempt-1", REV2, 7)
        .await
        .unwrap();

    let checkpoint = sessions.join(".transactional-upgrade/checkpoint.json");
    assert!(
        checkpoint.is_file(),
        "D must durably persist an independently reloadable checkpoint before G1 release"
    );
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&checkpoint).unwrap()).unwrap();
    value["generation"] = serde_json::json!(6);
    std::fs::write(&checkpoint, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    drop(first);

    let successor = ProjectExecutorSupervisor::create(&sessions, REV2).unwrap();
    let state = successor.inspect_transactional_upgrade().unwrap();
    assert_eq!(state.phase, UpgradePhase::Failed);
    assert!(!state.admissions_open);
    assert!(!state.checkpoint_verified);
    assert!(
        state
            .failure_reason
            .as_deref()
            .unwrap_or("")
            .contains("D_CHECKPOINT")
    );
}

#[tokio::test]
async fn completed_upgrade_rejects_stale_or_jumped_checkpoint_on_next_prepare() {
    for invalid_generation in [7_u64, 9_u64] {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        let sha = make_repo(&repo);
        let sessions = root.path().join("sessions");
        let id = "f3111111-2222-4333-8444-555555555555";
        let first = ProjectExecutorSupervisor::create(&sessions, REV1).unwrap();
        first
            .create_session(request(id, &repo, &sha))
            .await
            .unwrap();
        first
            .prepare_transactional_upgrade(id, "attempt-1", REV2, 7)
            .await
            .unwrap();
        drop(first);

        let second = ProjectExecutorSupervisor::create(&sessions, REV2).unwrap();
        let state = second.inspect_transactional_upgrade().unwrap();
        let payload = "checkpoint continuity payload";
        let resume = resume_intent(&state, "event-checkpoint-continuity", payload);
        let complete = second
            .resume_transactional_upgrade(resume, payload)
            .unwrap();
        assert_eq!(complete.request.checkpoint_generation, 8);

        let error = second
            .prepare_transactional_upgrade(id, "attempt-2", REV3, invalid_generation)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("D_CHECKPOINT_PREPARE_MISMATCH"));
        let still_complete = second.inspect_transactional_upgrade().unwrap();
        assert_eq!(still_complete.phase, UpgradePhase::Complete);
        assert!(still_complete.admissions_open);
        assert_eq!(still_complete.request.checkpoint_generation, 8);
    }
}
