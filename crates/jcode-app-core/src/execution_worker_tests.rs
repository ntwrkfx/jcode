use crate::execution_worker::{ExecutionCommand, ExecutionRequest, ExecutionWorker};

#[tokio::test]
async fn execution_worker_inspect_is_versioned_and_correlated() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let worker = ExecutionWorker::create(workspace.path())
        .await
        .expect("create execution worker");

    let response = worker
        .handle(ExecutionRequest {
            protocol: 1,
            id: "req-1".to_string(),
            command: ExecutionCommand::Inspect,
        })
        .await;

    assert_eq!(response.protocol, 1);
    assert_eq!(response.id, "req-1");
    assert!(response.ok);
    assert!(response.error.is_none());
    let result = response.result.expect("inspect result");
    assert_eq!(result["closed"], false);
    assert_eq!(
        result["workspace"].as_str(),
        workspace.path().canonicalize().unwrap().to_str()
    );
}

#[tokio::test]
async fn execution_worker_lists_deterministic_tools() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let worker = ExecutionWorker::create(workspace.path()).await.unwrap();
    let response = worker
        .handle(ExecutionRequest {
            protocol: 1,
            id: "req-tools".to_string(),
            command: ExecutionCommand::ToolList,
        })
        .await;

    assert!(response.ok);
    let tools = response.result.unwrap().as_array().unwrap().clone();
    let names: std::collections::BTreeSet<_> = tools
        .iter()
        .filter_map(|value| value.as_str())
        .collect();
    assert_eq!(names.len(), 9);
    assert!(names.contains("read"));
    assert!(names.contains("write"));
    assert!(names.contains("bash"));
}

#[tokio::test]
async fn execution_worker_dispatches_tool_call() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let worker = ExecutionWorker::create(workspace.path()).await.unwrap();
    let response = worker
        .handle(ExecutionRequest {
            protocol: 1,
            id: "req-write".to_string(),
            command: ExecutionCommand::ToolCall {
                tool: "write".to_string(),
                input: serde_json::json!({"file_path": "worker.txt", "content": "sample\n"}),
            },
        })
        .await;

    assert!(response.ok, "{:?}", response.error);
    assert_eq!(response.result.as_ref().unwrap()["title"], "worker.txt");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("worker.txt")).unwrap(),
        "sample\n"
    );
}

#[tokio::test]
async fn execution_worker_close_blocks_later_tool_calls() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let worker = ExecutionWorker::create(workspace.path()).await.unwrap();
    let closed = worker
        .handle(ExecutionRequest {
            protocol: 1,
            id: "req-close".to_string(),
            command: ExecutionCommand::Close,
        })
        .await;
    assert!(closed.ok);

    let response = worker
        .handle(ExecutionRequest {
            protocol: 1,
            id: "req-after-close".to_string(),
            command: ExecutionCommand::ToolCall {
                tool: "read".to_string(),
                input: serde_json::json!({"file_path": "missing.txt"}),
            },
        })
        .await;
    assert!(!response.ok);
    assert!(response.error.unwrap().contains("closed"));
}

#[tokio::test]
async fn execution_worker_rejects_unknown_protocol_version() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let worker = ExecutionWorker::create(workspace.path()).await.unwrap();
    let response = worker
        .handle(ExecutionRequest {
            protocol: 99,
            id: "req-version".to_string(),
            command: ExecutionCommand::Inspect,
        })
        .await;
    assert!(!response.ok);
    assert_eq!(response.protocol, 1);
    assert!(response.error.unwrap().contains("unsupported execution protocol"));
}
