use crate::execution_session::ExecutionSession;
use serde_json::json;
use std::collections::BTreeSet;

#[tokio::test]
async fn execution_session_binds_workspace_and_lists_execution_tools() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let expected_workspace = workspace.path().canonicalize().expect("canonical workspace");

    let session = ExecutionSession::create(workspace.path())
        .await
        .expect("create execution session");
    let info = session.inspect();

    assert!(!info.id.is_empty());
    assert_eq!(info.workspace, expected_workspace);
    assert!(!info.closed);

    let tools: BTreeSet<String> = session.tool_names().await.into_iter().collect();
    let expected: BTreeSet<String> = [
        "apply_patch", "bash", "batch", "edit", "ls", "multiedit", "patch", "read", "write",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert_eq!(tools, expected);
}

#[tokio::test]
async fn execution_session_dispatches_tool_in_bound_workspace() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let session = ExecutionSession::create(workspace.path())
        .await
        .expect("create execution session");

    session
        .call_tool(
            "write",
            json!({"file_path": "session.txt", "content": "model-less\n"}),
        )
        .await
        .expect("dispatch write tool");

    let content = std::fs::read_to_string(workspace.path().join("session.txt"))
        .expect("read session output");
    assert_eq!(content, "model-less\n");
}

#[tokio::test]
async fn closed_execution_session_rejects_tool_calls() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let session = ExecutionSession::create(workspace.path())
        .await
        .expect("create execution session");

    session.close();
    assert!(session.inspect().closed);

    let result = session
        .call_tool(
            "write",
            json!({"file_path": "should-not-exist.txt", "content": "blocked\n"}),
        )
        .await;
    assert!(result.is_err());
    assert!(result.err().unwrap().to_string().contains("closed"));
    assert!(!workspace.path().join("should-not-exist.txt").exists());
}

#[tokio::test]
async fn execution_session_rejects_unowned_background_bash() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let session = ExecutionSession::create(workspace.path())
        .await
        .expect("create execution session");

    let result = session
        .call_tool(
            "bash",
            json!({"command": "sleep 60", "run_in_background": true}),
        )
        .await;

    assert!(result.is_err());
    assert!(result.err().unwrap().to_string().contains("background"));
}

#[tokio::test]
async fn execution_session_timeout_does_not_promote_bash_to_global_background() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let session = ExecutionSession::create(workspace.path())
        .await
        .expect("create execution session");

    let result = session
        .call_tool(
            "bash",
            json!({
                "command": "bash -c 'sleep 0.2; touch timeout-leak.txt' & wait",
                "timeout": 25
            }),
        )
        .await;

    tokio::time::sleep(std::time::Duration::from_millis(350)).await;
    assert!(result.is_err(), "execution-session timeout must fail");
    assert!(
        !workspace.path().join("timeout-leak.txt").exists(),
        "timed-out child escaped execution-session ownership"
    );
}
