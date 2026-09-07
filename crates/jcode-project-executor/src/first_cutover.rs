use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const FIRST_CUTOVER_OBSERVATION_SCHEMA_VERSION: &str =
    "project-executor-first-cutover-observation/v1";
pub const FIRST_CUTOVER_ASSESSMENT_SCHEMA_VERSION: &str =
    "project-executor-first-cutover-assessment/v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirstCutoverObservation {
    pub schema_version: String,
    pub predecessor_revision: String,
    pub successor_revision: String,
    pub ready_session_count: usize,
    pub current_writer_custody_count: usize,
    pub unknown_session_count: usize,
    pub incomplete_transactional_upgrade: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirstCutoverAssessment {
    pub schema_version: String,
    pub authority_effect: String,
    pub result: String,
    pub predecessor_revision: String,
    pub successor_revision: String,
    pub reason_codes: Vec<String>,
}

pub fn assess_first_cutover(
    observation: &FirstCutoverObservation,
) -> Result<FirstCutoverAssessment> {
    if observation.schema_version != FIRST_CUTOVER_OBSERVATION_SCHEMA_VERSION {
        bail!("unsupported first-cutover observation schema");
    }
    validate_sha(&observation.predecessor_revision, "predecessor_revision")?;
    validate_sha(&observation.successor_revision, "successor_revision")?;
    if observation.predecessor_revision == observation.successor_revision {
        bail!("first cutover requires a distinct successor revision");
    }

    let mut reasons = Vec::new();
    if observation.ready_session_count > 0 {
        reasons.push("READY_SESSIONS_PRESENT".to_owned());
    }
    if observation.current_writer_custody_count > 0 {
        reasons.push("WRITER_CUSTODY_PRESENT".to_owned());
    }
    if observation.unknown_session_count > 0 {
        reasons.push("UNKNOWN_SESSION_STATE".to_owned());
    }
    if observation.incomplete_transactional_upgrade {
        reasons.push("TRANSACTIONAL_UPGRADE_INCOMPLETE".to_owned());
    }

    Ok(FirstCutoverAssessment {
        schema_version: FIRST_CUTOVER_ASSESSMENT_SCHEMA_VERSION.to_owned(),
        authority_effect: "NONE".to_owned(),
        result: if reasons.is_empty() {
            "READY_FOR_FIRST_CUTOVER".to_owned()
        } else {
            "BLOCKED".to_owned()
        },
        predecessor_revision: observation.predecessor_revision.clone(),
        successor_revision: observation.successor_revision.clone(),
        reason_codes: reasons,
    })
}

fn validate_sha(value: &str, field: &str) -> Result<()> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("{field} must be exactly 40 hexadecimal characters");
    }
    Ok(())
}
