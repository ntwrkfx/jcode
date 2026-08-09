use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn request(
    child: &mut std::process::Child,
    reader: &mut BufReader<std::process::ChildStdout>,
    value: Value,
) -> Value {
    let stdin = child.stdin.as_mut().unwrap();
    writeln!(stdin, "{}", serde_json::to_string(&value).unwrap()).unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

#[test]
fn worker_binary_runs_process_lifecycle_over_jsonl() {
    let workspace = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_jcode-project-executor-worker"))
        .args([
            "--workspace",
            workspace.path().to_str().unwrap(),
            "--state-root",
            state.path().to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    let start = request(
        &mut child,
        &mut reader,
        json!({
            "protocol": "project-executor/v1",
            "id": "1",
            "command": {"op": "process_start", "argv": ["printf", "worker-ok"]}
        }),
    );
    assert_eq!(start["ok"], true);
    let process_id = start["result"]["process_id"].as_str().unwrap().to_owned();

    let waited = request(
        &mut child,
        &mut reader,
        json!({
            "protocol": "project-executor/v1",
            "id": "2",
            "command": {"op": "process_wait", "process_id": process_id, "timeout_seconds": 2.0}
        }),
    );
    assert_eq!(waited["ok"], true);
    assert_eq!(waited["result"]["Exited"]["code"], 0);

    let read = request(
        &mut child,
        &mut reader,
        json!({
            "protocol": "project-executor/v1",
            "id": "3",
            "command": {"op": "process_read", "process_id": process_id}
        }),
    );
    assert_eq!(read["result"]["data"], "worker-ok");
    assert_eq!(read["result"]["eof"], true);

    let closed = request(
        &mut child,
        &mut reader,
        json!({
            "protocol": "project-executor/v1",
            "id": "4",
            "command": {"op": "close"}
        }),
    );
    assert_eq!(closed["ok"], true);
    assert_eq!(child.wait().unwrap().code(), Some(0));
}
