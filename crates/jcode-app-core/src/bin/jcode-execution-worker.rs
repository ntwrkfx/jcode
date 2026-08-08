use anyhow::{Result, bail};
use jcode_app_core::execution_worker::{ExecutionCommand, ExecutionRequest, ExecutionWorker};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

fn workspace_arg() -> Result<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    let flag = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing --cwd"))?;
    if flag.to_string_lossy() != "--cwd" {
        bail!("expected --cwd <workspace>");
    }
    let workspace = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing workspace after --cwd"))?;
    if args.next().is_some() {
        bail!("unexpected extra arguments");
    }
    Ok(PathBuf::from(workspace))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let worker = ExecutionWorker::create(workspace_arg()?).await?;
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let request: ExecutionRequest = serde_json::from_str(&line)?;
        let should_close = matches!(&request.command, ExecutionCommand::Close);
        let response = worker.handle(request).await;

        serde_json::to_writer(&mut stdout, &response)?;
        writeln!(stdout)?;
        stdout.flush()?;

        if should_close && response.ok {
            break;
        }
    }

    Ok(())
}
