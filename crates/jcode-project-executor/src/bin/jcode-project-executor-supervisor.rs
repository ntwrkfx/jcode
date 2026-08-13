use anyhow::{Context, Result, anyhow, bail};
use jcode_project_executor::{
    ExecutorRequest, ExecutorResponse, ProjectExecutorSupervisor, handle_executor_request,
};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

struct Args {
    session_root: PathBuf,
    worktree_root: PathBuf,
    socket: PathBuf,
    implementation_revision: String,
}

fn args() -> Result<Args> {
    let mut values = std::env::args_os().skip(1);
    let mut session_root = None;
    let mut worktree_root = None;
    let mut socket = None;
    let mut implementation_revision = None;
    while let Some(flag) = values.next() {
        let value = values
            .next()
            .ok_or_else(|| anyhow!("missing value for {:?}", flag))?;
        match flag.to_string_lossy().as_ref() {
            "--session-root" => session_root = Some(PathBuf::from(value)),
            "--worktree-root" => worktree_root = Some(PathBuf::from(value)),
            "--socket" => socket = Some(PathBuf::from(value)),
            "--implementation-revision" => {
                implementation_revision = Some(value.to_string_lossy().into_owned())
            }
            other => bail!("unknown argument: {other}"),
        }
    }
    let session_root = session_root.ok_or_else(|| anyhow!("missing --session-root"))?;
    let worktree_root = worktree_root.unwrap_or_else(|| {
        session_root
            .parent()
            .unwrap_or_else(|| std::path::Path::new("/"))
            .to_path_buf()
    });
    Ok(Args {
        session_root,
        worktree_root,
        socket: socket.ok_or_else(|| anyhow!("missing --socket"))?,
        implementation_revision: implementation_revision
            .ok_or_else(|| anyhow!("missing --implementation-revision"))?,
    })
}

fn prepare_socket(path: &PathBuf) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create supervisor socket directory")?;
    }
    if path.exists() {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.file_type().is_socket() {
            bail!("refusing to remove non-socket path: {}", path.display());
        }
        if StdUnixStream::connect(path).is_ok() {
            bail!("supervisor socket is already active: {}", path.display());
        }
        std::fs::remove_file(path).context("remove stale supervisor socket")?;
    }
    Ok(())
}

async fn handle_connection(
    stream: UnixStream,
    supervisor: Arc<ProjectExecutorSupervisor>,
) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<ExecutorRequest>(&line) {
            Ok(request) => handle_executor_request(&supervisor, request).await,
            Err(error) => {
                ExecutorResponse::failure(String::new(), format!("invalid request: {error}"))
            }
        };
        let mut encoded = serde_json::to_vec(&response)?;
        encoded.push(b'\n');
        write_half.write_all(&encoded).await?;
        write_half.flush().await?;
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = args()?;
    prepare_socket(&args.socket)?;
    let supervisor = Arc::new(ProjectExecutorSupervisor::create_with_worktree_root(
        &args.session_root,
        &args.worktree_root,
        args.implementation_revision,
    )?);
    let listener = UnixListener::bind(&args.socket).context("bind supervisor socket")?;
    std::fs::set_permissions(&args.socket, std::fs::Permissions::from_mode(0o600))?;

    loop {
        let (stream, _) = listener.accept().await?;
        let supervisor = Arc::clone(&supervisor);
        tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, supervisor).await {
                eprintln!("project executor connection error: {error:#}");
            }
        });
    }
}
