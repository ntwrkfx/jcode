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
