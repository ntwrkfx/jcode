//! Bounded model-less project executor derived from jcode execution mechanics.

pub mod custody;
mod durable;
pub mod effect;
mod first_cutover;
mod process;
mod protocol;
mod recovery;
pub mod retirement;
mod service;
mod session;
mod supervisor;
pub mod transactional_upgrade;
mod worker;

pub use first_cutover::{
    FirstCutoverAssessment, FirstCutoverObservation, assess_first_cutover,
};
pub use process::{
    ExecutionProcessManager, ExecutionProcessOutput, ExecutionProcessRef, ExecutionProcessState,
};
pub use protocol::{
    ExecutorCommand, ExecutorRequest, ExecutorResponse, WorkerCommand, WorkerRequest,
    WorkerResponse,
};
pub use recovery::{
    DecodedSessionRecord, MaterialObservation, RecoveryDecision, RecoveryEvidence, RecoveryHealth,
    RecoveryReasonCode, RecoverySummary, SessionRecoveryReceipt, decode_session_record,
    derive_runnability, observe_git_material, summarize_recovery,
};
pub use service::handle_executor_request;
pub use session::{
    AccessMode, CustodyAssessment, CustodyGenerationEvidence, CustodyState, ExpectedMaterial,
    LocalMaterialState, SessionCreateRequest, SessionInspection, SessionLifecycle, SessionRecord,
    SessionRunnability, SessionState, WorkspaceOrigin, WorktreeBinding, WorktreeInspection,
    WorktreeMode, WorktreeOwnership,
};
pub use supervisor::ProjectExecutorSupervisor;
pub use transactional_upgrade::{
    ResumeIntent, TransactionalUpgradeRequest, TransactionalUpgradeState,
    TransactionalUpgradeStore, UpgradePhase, event_digest,
};
pub use worker::ExecutionWorker;
