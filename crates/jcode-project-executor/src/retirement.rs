use crate::effect::EffectAuthorizationBinding;
use crate::session::{CustodyGenerationEvidence, ExpectedMaterial};
use serde::{Deserialize, Serialize};

pub const WORKSPACE_RETIRE_EFFECT_CLASS: &str = "WORKSPACE_RETIRE";
pub const RETIREMENT_RECEIPT_SCHEMA_VERSION: &str = "RetirementReceipt/v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RetirementDisposition {
    DiscardCleanManagedWorkspace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RetirementOutcome {
    Denied,
    Retired,
    Failed,
    AlreadyAbsentObserved,
    OutcomeAmbiguous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RetirementReasonCode {
    NotManagedHarness,
    SessionNotClosed,
    MaterialPresent,
    AuthorizationMissing,
    ScopeMismatch,
    CustodyNotCurrent,
    MaterialUnknown,
    MaterialChanged,
    EffectFailed,
    OutcomeAmbiguous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetirementIntent {
    pub retirement_intent_id: String,
    pub work_id: String,
    pub execution_id: String,
    pub resource_identity: String,
    pub workspace_identity: String,
    pub candidate_revision: String,
    pub effect_class: String,
    pub authorization_digest: String,
    pub authorization_binding: Option<EffectAuthorizationBinding>,
    pub expected_material: ExpectedMaterial,
    pub disposition: RetirementDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetirementReceipt {
    pub schema_version: String,
    pub intent: RetirementIntent,
    pub outcome: RetirementOutcome,
    pub reason: Option<RetirementReasonCode>,
    pub custody_generation: Option<CustodyGenerationEvidence>,
    pub observed_at: u64,
}
