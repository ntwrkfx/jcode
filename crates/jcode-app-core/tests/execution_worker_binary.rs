use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn send(stdin: &mut impl Write, reader: &mut impl BufRead, request: &str) -> Value {
    writeln!(stdin, "{request}").expect("write request");
    stdin.flush().expect("flush request");

    let mut line = String::new();
    reader.read_line(&mut line).expect("read response");
    assert!(!line.is_empty(), "worker closed before responding");
    serde_json::from_str(&line).expect("valid JSON response")
}

#[test]
fn jsonl_worker_handles_inspect_tool_call_and_close() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let mut child = Command::new(env!("CARGO_BIN_EXE_jcode-execution-worker"))
        .arg("--cwd")
        .arg(workspace.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn execution worker");

    let mut stdin = child.stdin.take().expect("worker stdin");
    let stdout = child.stdout.take().expect("worker stdout");
    let mut reader = BufReader::new(stdout);
    let inspect = send(
        &mut stdin,
        &mut reader,
        r#"{"protocol":1,"id":"inspect-1","command":{"type":"inspect"}}"#,
    );
    assert_eq!(inspect["id"], "inspect-1");
    assert_eq!(inspect["ok"], true);
    assert_eq!(
        inspect["result"]["workspace"],
        workspace
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );

    let write = send(
        &mut stdin,
        &mut reader,
        r#"{"protocol":1,"id":"write-1","command":{"type":"tool_call","tool":"write","input":{"file_path":"rpc.txt","content":"jsonl\n"}}}"#,
    );
    assert_eq!(write["id"], "write-1");
    assert_eq!(write["ok"], true);
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("rpc.txt")).unwrap(),
        "jsonl\n"
    );

    let close = send(
        &mut stdin,
        &mut reader,
        r#"{"protocol":1,"id":"close-1","command":{"type":"close"}}"#,
    );
    assert_eq!(close["id"], "close-1");
    assert_eq!(close["ok"], true);

    drop(stdin);
    let status = child.wait().expect("wait for worker");
    assert!(status.success(), "worker exit status: {status}");
}
