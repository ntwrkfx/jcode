use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const WRITE_PROCESS_EFFECT_CLASS: &str = "PROCESS_START_WRITE";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectAuthorizationBinding {
    pub work_id: String,
    pub execution_id: String,
    pub resource_identity: String,
    pub workspace_identity: String,
    pub candidate_revision: String,
    pub effect_class: String,
    pub authorization_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialAuthorizationClaim {
    pub work_id: String,
    pub execution_id: String,
    pub resource_identity: String,
    pub workspace_identity: String,
    pub candidate_revision: String,
    pub effect_class: String,
    pub authorization_digest: String,
}

impl EffectAuthorizationBinding {
    pub fn validate(&self, claim: &MaterialAuthorizationClaim) -> Result<()> {
        validate_authorization_digest(&self.authorization_digest)?;
        validate_authorization_digest(&claim.authorization_digest)?;
        if self.work_id != claim.work_id {
            bail!("EFFECT_WORK_ID_MISMATCH");
        }
        if self.execution_id != claim.execution_id {
            bail!("EFFECT_EXECUTION_ID_MISMATCH");
        }
        if self.resource_identity != claim.resource_identity {
            bail!("EFFECT_RESOURCE_IDENTITY_MISMATCH");
        }
        if self.workspace_identity != claim.workspace_identity {
            bail!("EFFECT_WORKSPACE_IDENTITY_MISMATCH");
        }
        if self.candidate_revision != claim.candidate_revision {
            bail!("EFFECT_CANDIDATE_REVISION_MISMATCH");
        }
        if self.effect_class != claim.effect_class {
            bail!("EFFECT_CLASS_MISMATCH");
        }
        if self.authorization_digest != claim.authorization_digest {
            bail!("EFFECT_AUTHORIZATION_DIGEST_MISMATCH");
        }
        Ok(())
    }
}

pub fn validate_authorization_digest(value: &str) -> Result<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("EFFECT_AUTHORIZATION_DIGEST_INVALID");
    }
    Ok(())
}
