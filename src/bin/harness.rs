use anyhow::Result;
use clap::Parser;
use jcode::id::new_id;
use jcode::tool::{Registry, ToolContext, ToolExecutionMode};
use serde_json::json;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "jcode-harness")]
#[command(about = "Run a deterministic tool harness smoke test")]
struct Args {
    /// Use an explicit working directory (defaults to a temp folder).
    #[arg(long)]
    cwd: Option<String>,
}

struct ToolCase {
    name: &'static str,
    input: serde_json::Value,
    label: &'static str,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let workspace = if let Some(cwd) = args.cwd {
        PathBuf::from(cwd)
    } else {
        create_temp_workspace()?
    };

    std::fs::create_dir_all(&workspace)?;
    std::env::set_current_dir(&workspace)?;
    eprintln!("Harness workspace: {}", workspace.display());

    let registry = Registry::execution().await;

    let session_id = new_id("harness");
    let base_ctx = ToolContext {
        session_id: session_id.clone(),
        message_id: session_id.clone(),
        tool_call_id: String::new(),
        working_dir: Some(workspace.clone()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    };

    let mut cases = Vec::new();
    cases.push(ToolCase {
        name: "write",
        label: "write sample.txt",
        input: json!({"file_path": "sample.txt", "content": "alpha\nbeta\n"}),
    });
    cases.push(ToolCase {
        name: "read",
        label: "read sample.txt",
        input: json!({"file_path": "sample.txt"}),
    });
    cases.push(ToolCase {
        name: "edit",
        label: "edit sample.txt (alpha -> alpha1)",
        input: json!({"file_path": "sample.txt", "old_string": "alpha", "new_string": "alpha1"}),
    });
    cases.push(ToolCase {
        name: "multiedit",
        label: "multiedit sample.txt",
        input: json!({
            "file_path": "sample.txt",
            "edits": [
                {"old_string": "alpha1", "new_string": "alpha2"},
                {"old_string": "beta", "new_string": "beta1"}
            ]
        }),
    });
    cases.push(ToolCase {
        name: "patch",
        label: "patch sample.txt",
        input: json!({"patch_text": "--- a/sample.txt\n+++ b/sample.txt\n@@ -1,2 +1,3 @@\n alpha2\n beta1\n+gamma\n"}),
    });
    cases.push(ToolCase {
        name: "apply_patch",
        label: "apply_patch add file",
        input: json!({"patch_text": "*** Begin Patch\n*** Add File: added.txt\n+added\n*** End Patch\n"}),
    });
    cases.push(ToolCase {
        name: "ls",
        label: "ls .",
        input: json!({"path": "."}),
    });
    cases.push(ToolCase {
        name: "bash",
        label: "bash pwd",
        input: json!({"command": "pwd"}),
    });
    cases.push(ToolCase {
        name: "batch",
        label: "batch ls + read",
        input: json!({
            "tool_calls": [
                {"tool": "ls", "parameters": {"path": "."}},
                {"tool": "read", "parameters": {"file_path": "sample.txt"}}
            ]
        }),
    });

    for (idx, case) in cases.iter().enumerate() {
        let ctx = ToolContext {
            tool_call_id: format!("harness-{}", idx + 1),
            ..base_ctx.clone()
        };
        println!("\n== {} ({}) ==", case.name, case.label);
        match registry.execute(case.name, case.input.clone(), ctx).await {
            Ok(output) => {
                if let Some(title) = output.title {
                    println!("[title] {}", title);
                }
                println!("{}", output.output);
            }
            Err(err) => {
                println!("[error] {}", err);
            }
        }
    }

    Ok(())
}

fn create_temp_workspace() -> Result<PathBuf> {
    let mut path = std::env::temp_dir();
    path.push(format!("jcode-harness-{}", new_id("run")));
    Ok(path)
}
