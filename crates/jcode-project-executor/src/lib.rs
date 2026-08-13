//! Bounded model-less project executor derived from jcode execution mechanics.

mod process;
mod protocol;
mod service;
mod session;
mod supervisor;
mod worker;

pub use process::{
    ExecutionProcessManager, ExecutionProcessOutput, ExecutionProcessRef, ExecutionProcessState,
};
pub use protocol::{
    ExecutorCommand, ExecutorRequest, ExecutorResponse, WorkerCommand, WorkerRequest,
    WorkerResponse,
};
pub use service::handle_executor_request;
pub use session::{
    AccessMode, SessionCreateRequest, SessionInspection, SessionRecord, SessionState,
    WorktreeBinding, WorktreeInspection, WorktreeMode, WorktreeOwnership,
};
pub use supervisor::ProjectExecutorSupervisor;
pub use worker::ExecutionWorker;
