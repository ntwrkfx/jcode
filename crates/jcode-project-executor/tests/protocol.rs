use jcode_project_executor::WorkerCommand;

#[test]
fn protocol_accepts_argv_process_start() {
    let command: WorkerCommand = serde_json::from_value(serde_json::json!({
        "op": "process_start",
        "argv": ["git", "status", "--porcelain"],
        "cwd": "."
    }))
    .unwrap();

    match command {
        WorkerCommand::ProcessStart { argv, cwd } => {
            assert_eq!(argv, vec!["git", "status", "--porcelain"]);
            assert_eq!(cwd.as_deref(), Some("."));
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn protocol_rejects_shell_string_process_start() {
    let error = serde_json::from_value::<WorkerCommand>(serde_json::json!({
        "op": "process_start",
        "command": "git status --porcelain"
    }))
    .unwrap_err();
    assert!(error.to_string().contains("argv"));
}

#[test]
fn supervisor_protocol_accepts_session_create() {
    use jcode_project_executor::{ExecutorCommand, ExecutorRequest};
    let request: ExecutorRequest = serde_json::from_value(serde_json::json!({
        "protocol": "project-executor/v1",
        "id": "create-1",
        "command": {
            "op": "session_create",
            "execution_id": "77777777-7777-4777-8777-777777777777",
            "work_identity": "local:test:protocol",
            "repository": "/srv/repo",
            "base_sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        }
    }))
    .unwrap();
    assert!(matches!(
        request.command,
        ExecutorCommand::SessionCreate { .. }
    ));
}

#[test]
fn supervisor_protocol_rejects_shell_string_process_start() {
    use jcode_project_executor::ExecutorRequest;
    let error = serde_json::from_value::<ExecutorRequest>(serde_json::json!({
        "protocol": "project-executor/v1",
        "id": "bad-start",
        "command": {"op": "process_start", "execution_id": "x", "command": "git status"}
    }))
    .unwrap_err();
    assert!(error.to_string().contains("argv"));
}
