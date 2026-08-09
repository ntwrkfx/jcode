use std::time::Duration;

use jcode_project_executor::{ExecutionProcessManager, ExecutionProcessState};

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

#[tokio::test]
async fn starts_argv_waits_and_reads_output() {
    let workspace = tempfile::tempdir().unwrap();
    let manager = ExecutionProcessManager::create(workspace.path()).unwrap();
    let process = manager
        .start(
            argv(&["bash", "-c", "printf 'alpha\\nbeta\\n'; exit 7"]),
            None,
        )
        .await
        .unwrap();

    let state = manager
        .wait(&process.process_id, Some(Duration::from_secs(2)))
        .await
        .unwrap();
    assert_eq!(state, ExecutionProcessState::Exited { code: Some(7) });

    let output = manager.read(&process.process_id, 0, 1024).await.unwrap();
    assert_eq!(output.data, "alpha\nbeta\n");
    assert!(output.eof);
}

#[tokio::test]
async fn rejects_empty_argv_and_workspace_escape() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let outside = root.path().join("outside");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let manager = ExecutionProcessManager::create(&workspace).unwrap();

    let empty = manager.start(vec![], None).await.unwrap_err();
    assert!(empty.to_string().contains("argv"));

    let escaped = manager
        .start(argv(&["pwd"]), Some("../outside".to_owned()))
        .await
        .unwrap_err();
    assert!(escaped.to_string().contains("outside workspace"));
}

#[tokio::test]
async fn read_is_cursor_bounded() {
    let workspace = tempfile::tempdir().unwrap();
    let manager = ExecutionProcessManager::create(workspace.path()).unwrap();
    let process = manager
        .start(argv(&["printf", "abcdef"]), None)
        .await
        .unwrap();
    manager.wait(&process.process_id, None).await.unwrap();

    let first = manager.read(&process.process_id, 0, 3).await.unwrap();
    assert_eq!(first.data, "abc");
    assert_eq!(first.next_offset, 3);
    assert!(!first.eof);

    let second = manager.read(&process.process_id, 3, 3).await.unwrap();
    assert_eq!(second.data, "def");
    assert_eq!(second.next_offset, 6);
    assert!(second.eof);
}

#[tokio::test]
async fn wait_timeout_keeps_process_owned_and_abortable() {
    let workspace = tempfile::tempdir().unwrap();
    let manager = ExecutionProcessManager::create(workspace.path()).unwrap();
    let process = manager.start(argv(&["sleep", "5"]), None).await.unwrap();

    let error = manager
        .wait(&process.process_id, Some(Duration::from_millis(10)))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("timed out"));

    let state = manager.abort(&process.process_id).await.unwrap();
    assert_eq!(state, ExecutionProcessState::Aborted);
}

#[tokio::test]
async fn abort_kills_owned_descendants() {
    let workspace = tempfile::tempdir().unwrap();
    let manager = ExecutionProcessManager::create(workspace.path()).unwrap();
    let process = manager
        .start(
            argv(&["bash", "-c", "(sleep 0.2; touch abort-leak.txt) & wait"]),
            None,
        )
        .await
        .unwrap();

    let state = manager.abort(&process.process_id).await.unwrap();
    assert_eq!(state, ExecutionProcessState::Aborted);
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert!(!workspace.path().join("abort-leak.txt").exists());
}

#[tokio::test]
async fn close_all_kills_every_owned_process() {
    let workspace = tempfile::tempdir().unwrap();
    let manager = ExecutionProcessManager::create(workspace.path()).unwrap();
    for name in ["close-a.txt", "close-b.txt"] {
        manager
            .start(
                argv(&["bash", "-c", &format!("(sleep 0.2; touch {name}) & wait")]),
                None,
            )
            .await
            .unwrap();
    }

    manager.close_all().await.unwrap();
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert!(!workspace.path().join("close-a.txt").exists());
    assert!(!workspace.path().join("close-b.txt").exists());
}
