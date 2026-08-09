//! Bounded model-less project executor derived from jcode execution mechanics.

mod process;
mod protocol;
mod worker;

pub use process::{
    ExecutionProcessManager, ExecutionProcessOutput, ExecutionProcessRef, ExecutionProcessState,
};
pub use protocol::{WorkerCommand, WorkerRequest, WorkerResponse};
pub use worker::ExecutionWorker;
