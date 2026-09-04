use anyhow::{Context, Result};
use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use uuid::Uuid;

pub(crate) fn durable_write_json<T: Serialize>(target: &Path, value: &T) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| anyhow::anyhow!("durable target has no parent"))?;
    std::fs::create_dir_all(parent).context("create durable parent directory")?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        target
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("state"),
        Uuid::new_v4()
    ));
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .context("open durable temporary file")?;
    file.write_all(&bytes)
        .context("write durable temporary file")?;
    file.sync_all().context("sync durable temporary file")?;
    drop(file);
    std::fs::rename(&temporary, target).context("commit durable state")?;
    File::open(parent)
        .context("open durable parent directory")?
        .sync_all()
        .context("sync durable parent directory")?;
    Ok(())
}
