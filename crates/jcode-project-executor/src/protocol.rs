use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const EXECUTOR_PROTOCOL_VERSION: &str = "project-executor/v1";

fn default_limit() -> usize {
    65_536
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerCommand {
    ProcessStart {
        argv: Vec<String>,
        cwd: Option<String>,
    },
    ProcessRead {
        process_id: String,
        #[serde(default)]
        offset: u64,
        #[serde(default = "default_limit")]
        limit: usize,
    },
    ProcessWait {
        process_id: String,
        timeout_seconds: Option<f64>,
    },
    ProcessAbort {
        process_id: String,
    },
    Close,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRequest {
    pub protocol: String,
    pub id: String,
    pub command: WorkerCommand,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerResponse {
    pub protocol: String,
    pub id: String,
    pub ok: bool,
    pub result: Option<Value>,
    pub error: Option<String>,
}

impl WorkerResponse {
    pub fn success(id: String, result: Value) -> Self {
        Self {
            protocol: EXECUTOR_PROTOCOL_VERSION.to_owned(),
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn failure(id: String, error: impl Into<String>) -> Self {
        Self {
            protocol: EXECUTOR_PROTOCOL_VERSION.to_owned(),
            id,
            ok: false,
            result: None,
            error: Some(error.into()),
        }
    }
}
