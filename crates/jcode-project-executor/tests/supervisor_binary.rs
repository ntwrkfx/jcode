use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const REVISION: &str = "dddddddddddddddddddddddddddddddddddddddd";
const DEVICE_ID: &str = "11111111-1111-4111-8111-111111111111";

#[test]
fn supervisor_requires_explicit_valid_device_id() {
    let root = tempfile::tempdir().unwrap();
    let sessions = root.path().join("sessions");
    let socket = root.path().join("executor.sock");

    let mut missing = Command::new(env!("CARGO_BIN_EXE_jcode-project-executor-supervisor"))
        .args(["--session-root", sessions.to_str().unwrap()])
        .args(["--socket", socket.to_str().unwrap()])
        .args(["--implementation-revision", REVISION])
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    if missing.try_wait().unwrap().is_none() {
        missing.kill().unwrap();
        missing.wait().unwrap();
        panic!("supervisor started without --device-id");
    }

    let invalid = Command::new(env!("CARGO_BIN_EXE_jcode-project-executor-supervisor"))
        .args(["--session-root", sessions.to_str().unwrap()])
        .args(["--socket", socket.to_str().unwrap()])
        .args(["--implementation-revision", REVISION])
        .args(["--device-id", "not-a-uuid"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("invalid --device-id"));
}

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
    git(path, &["config", "user.name", "Executor Test"]);
    git(path, &["config", "user.email", "executor@example.invalid"]);
    std::fs::write(path.join("README.md"), "first\n").unwrap();
    git(path, &["add", "README.md"]);
    git(path, &["commit", "-q", "-m", "first"]);
    git(path, &["rev-parse", "HEAD"])
}

fn spawn_supervisor(session_root: &Path, socket: &Path) -> Child {
    let mut child = Command::new(env!("CARGO_BIN_EXE_jcode-project-executor-supervisor"))
        .args(["--session-root", session_root.to_str().unwrap()])
        .args(["--socket", socket.to_str().unwrap()])
        .args(["--implementation-revision", REVISION])
        .args(["--device-id", DEVICE_ID])
        .env("PROJECT_EXECUTOR_SECRET_TEST", "ambient-secret")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if UnixStream::connect(socket).is_ok() {
            break;
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "supervisor exited before readiness"
        );
        assert!(Instant::now() < deadline, "supervisor socket not ready");
        std::thread::sleep(Duration::from_millis(10));
    }
    child
}

fn send(socket: &Path, value: Value) -> Value {
    let mut stream = UnixStream::connect(socket).unwrap();
    writeln!(stream, "{}", serde_json::to_string(&value).unwrap()).unwrap();
    stream.flush().unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

#[test]
fn process_survives_controller_disconnect_and_reconnect() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let sessions = root.path().join("sessions");
    let socket = root.path().join("executor.sock");
    let mut supervisor = spawn_supervisor(&sessions, &socket);
    let id = "55555555-5555-4555-8555-555555555555";

    let created = send(
        &socket,
        json!({
            "protocol": "project-executor/v1", "id": "create",
            "command": {"op": "session_create", "execution_id": id,
                "work_identity": "local:test:disconnect", "repository": repo, "base_sha": sha}
        }),
    );
    assert_eq!(created["ok"], true);

    let started = send(
        &socket,
        json!({
            "protocol": "project-executor/v1", "id": "start",
            "command": {"op": "process_start", "execution_id": id,
                "argv": ["bash", "-c", "test -z \"$PROJECT_EXECUTOR_SECRET_TEST\" && printf started; sleep 0.15; printf done"]}
        }),
    );
    assert_eq!(started["ok"], true);
    let process_id = started["result"]["process_id"].as_str().unwrap().to_owned();
    std::thread::sleep(Duration::from_millis(250));
    let waited = send(
        &socket,
        json!({
            "protocol": "project-executor/v1", "id": "wait",
            "command": {"op": "process_wait", "execution_id": id,
                "process_id": process_id, "timeout_seconds": 1.0}
        }),
    );
    assert_eq!(waited["ok"], true);
    let read = send(
        &socket,
        json!({
            "protocol": "project-executor/v1", "id": "read",
            "command": {"op": "process_read", "execution_id": id,
                "process_id": process_id, "offset": 0, "limit": 64}
        }),
    );
    assert_eq!(read["result"]["data"], "starteddone");

    let closed = send(
        &socket,
        json!({
            "protocol": "project-executor/v1", "id": "close",
            "command": {"op": "session_close", "execution_id": id}
        }),
    );
    assert_eq!(closed["ok"], true);
    supervisor.kill().unwrap();
    supervisor.wait().unwrap();
}

#[test]
fn supervisor_restart_recovers_ready_session_catalog() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let sha = make_repo(&repo);
    let sessions = root.path().join("sessions");
    let socket = root.path().join("executor.sock");
    let id = "66666666-6666-4666-8666-666666666666";

    let mut first = spawn_supervisor(&sessions, &socket);
    let created = send(
        &socket,
        json!({
            "protocol": "project-executor/v1", "id": "create",
            "command": {"op": "session_create", "execution_id": id,
                "work_identity": "local:test:restart", "repository": repo, "base_sha": sha}
        }),
    );
    assert_eq!(created["ok"], true);
    first.kill().unwrap();
    first.wait().unwrap();

    let mut second = spawn_supervisor(&sessions, &socket);
    let inspected = send(
        &socket,
        json!({
            "protocol": "project-executor/v1", "id": "inspect",
            "command": {"op": "session_inspect", "execution_id": id}
        }),
    );
    assert_eq!(inspected["ok"], true);
    assert_eq!(inspected["result"]["execution_id"], id);
    assert_eq!(inspected["result"]["head_sha"], sha);
    assert_eq!(inspected["result"]["state"], "READY");
    let process = send(
        &socket,
        json!({
            "protocol": "project-executor/v1", "id": "start2",
            "command": {"op": "process_start", "execution_id": id,
                "argv": ["printf", "recovered"]}
        }),
    );
    assert_eq!(process["ok"], true);
    let process_id = process["result"]["process_id"].as_str().unwrap().to_owned();
    let waited = send(
        &socket,
        json!({
            "protocol": "project-executor/v1", "id": "wait2",
            "command": {"op": "process_wait", "execution_id": id,
                "process_id": process_id, "timeout_seconds": 1.0}
        }),
    );
    assert_eq!(waited["ok"], true);
    let closed = send(
        &socket,
        json!({
            "protocol": "project-executor/v1", "id": "close2",
            "command": {"op": "session_close", "execution_id": id}
        }),
    );
    assert_eq!(closed["ok"], true);
    second.kill().unwrap();
    second.wait().unwrap();
}

#[test]
fn duplicate_supervisor_refuses_active_socket() {
    let root = tempfile::tempdir().unwrap();
    let sessions = root.path().join("sessions");
    let socket = root.path().join("executor.sock");
    let mut first = spawn_supervisor(&sessions, &socket);
    let output = Command::new(env!("CARGO_BIN_EXE_jcode-project-executor-supervisor"))
        .args(["--session-root", sessions.to_str().unwrap()])
        .args(["--socket", socket.to_str().unwrap()])
        .args(["--implementation-revision", REVISION])
        .args(["--device-id", DEVICE_ID])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("already active"));
    assert!(UnixStream::connect(&socket).is_ok());
    first.kill().unwrap();
    first.wait().unwrap();
}
