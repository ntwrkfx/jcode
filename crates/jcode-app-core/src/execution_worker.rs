use crate::execution_session::ExecutionSession;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;

pub const EXECUTION_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutionCommand {
    Inspect,
    ToolList,
    ToolCall { tool: String, input: Value },
    Close,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExecutionRequest {
    pub protocol: u32,
    pub id: String,
    pub command: ExecutionCommand,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExecutionResponse {
    pub protocol: u32,
    pub id: String,
    pub ok: bool,
    pub result: Option<Value>,
    pub error: Option<String>,
}

pub struct ExecutionWorker {
    session: ExecutionSession,
}

impl ExecutionWorker {
    pub async fn create(workspace: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            session: ExecutionSession::create(workspace).await?,
        })
    }

    pub async fn handle(&self, request: ExecutionRequest) -> ExecutionResponse {
        if request.protocol != EXECUTION_PROTOCOL_VERSION {
            return ExecutionResponse {
                protocol: EXECUTION_PROTOCOL_VERSION,
                id: request.id,
                ok: false,
                result: None,
                error: Some(format!(
                    "unsupported execution protocol: {}",
                    request.protocol
                )),
            };
        }

        let result = match request.command {
            ExecutionCommand::Inspect => {
                let info = self.session.inspect();
                Ok(json!({
                    "id": info.id,
                    "workspace": info.workspace,
                    "closed": info.closed,
                }))
            }
            ExecutionCommand::ToolList => Ok(json!(self.session.tool_names().await)),
            ExecutionCommand::ToolCall { tool, input } => {
                match self.session.call_tool(&tool, input).await {
                    Ok(output) => {
                        let images: Vec<Value> = output
                            .images
                            .into_iter()
                            .map(|image| {
                                json!({
                                    "media_type": image.media_type,
                                    "data": image.data,
                                    "label": image.label,
                                })
                            })
                            .collect();
                        Ok(json!({
                            "output": output.output,
                            "title": output.title,
                            "metadata": output.metadata,
                            "images": images,
                        }))
                    }
                    Err(error) => Err(error),
                }
            }
            ExecutionCommand::Close => {
                self.session.close();
                Ok(json!({ "closed": true }))
            }
        };

        match result {
            Ok(result) => ExecutionResponse {
                protocol: EXECUTION_PROTOCOL_VERSION,
                id: request.id,
                ok: true,
                result: Some(result),
                error: None,
            },
            Err(error) => ExecutionResponse {
                protocol: EXECUTION_PROTOCOL_VERSION,
                id: request.id,
                ok: false,
                result: None,
                error: Some(error.to_string()),
            },
        }
    }
}
