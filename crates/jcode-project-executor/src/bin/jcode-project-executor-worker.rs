use anyhow::{Result, anyhow, bail};
use jcode_project_executor::{ExecutionWorker, WorkerCommand, WorkerRequest, WorkerResponse};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

struct Args {
    workspace: PathBuf,
    state_root: Option<PathBuf>,
}

fn args() -> Result<Args> {
    let mut values = std::env::args_os().skip(1);
    let mut workspace = None;
    let mut state_root = None;
    while let Some(flag) = values.next() {
        match flag.to_string_lossy().as_ref() {
            "--workspace" => {
                workspace = Some(PathBuf::from(
                    values.next().ok_or_else(|| anyhow!("missing workspace"))?,
                ))
            }
            "--state-root" => {
                state_root = Some(PathBuf::from(
                    values.next().ok_or_else(|| anyhow!("missing state root"))?,
                ))
            }
            other => bail!("unknown argument: {other}"),
        }
    }
    Ok(Args {
        workspace: workspace.ok_or_else(|| anyhow!("missing --workspace"))?,
        state_root,
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = args()?;
    let worker = match args.state_root {
        Some(root) => ExecutionWorker::create_with_state_root(&args.workspace, root)?,
        None => ExecutionWorker::create(&args.workspace)?,
    };
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let (response, should_close) = match serde_json::from_str::<WorkerRequest>(&line) {
            Ok(request) => {
                let should_close = matches!(&request.command, WorkerCommand::Close);
                (worker.handle(request).await, should_close)
            }
            Err(error) => (
                WorkerResponse::failure(String::new(), format!("invalid request: {error}")),
                false,
            ),
        };
        serde_json::to_writer(&mut stdout, &response)?;
        writeln!(stdout)?;
        stdout.flush()?;
        if should_close && response.ok {
            break;
        }
    }
    Ok(())
}
