use trogon_audit::{AuditName, AuditNameError, WriterId};
use trogon_nats::{NatsToken, SubjectTokenViolationError};
use trogon_std::env::ReadEnv;

use crate::constants::{
    AUDIT_WRITER_ID_PREFIX, ENV_GATEWAY_AUDIT_INSTANCE, ENV_GATEWAY_AUDIT_PROJECT, ENV_GATEWAY_AUDIT_TENANT,
};

/// Whose records these are and which process writes them (ADR#0086 Decision 1).
///
/// The gateway serves one tenant and project per process until a verified
/// credential carries them, so they come from configuration. There is no default
/// for any of the three: a record that names no tenant is not a record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformAuditConfig {
    pub tenant: AuditName,
    pub project: NatsToken,
    /// Derived from the instance name, so a restarted replica is the same writer.
    pub writer: WriterId,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PlatformAuditConfigError {
    #[error("{0} must be set when audit publishing is enabled")]
    Missing(&'static str),
    #[error("{ENV_GATEWAY_AUDIT_TENANT} is not a valid audit name")]
    Tenant(#[source] AuditNameError),
    #[error("{ENV_GATEWAY_AUDIT_PROJECT} is not a valid subject token")]
    Project(#[source] SubjectTokenViolationError),
    #[error("{ENV_GATEWAY_AUDIT_INSTANCE} is not a valid audit name")]
    Instance(#[source] AuditNameError),
}

impl PlatformAuditConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Result<Self, PlatformAuditConfigError> {
        let tenant = required(env, ENV_GATEWAY_AUDIT_TENANT)?;
        let project = required(env, ENV_GATEWAY_AUDIT_PROJECT)?;
        let instance = required(env, ENV_GATEWAY_AUDIT_INSTANCE)?;
        Ok(Self {
            tenant: AuditName::new(tenant).map_err(PlatformAuditConfigError::Tenant)?,
            project: NatsToken::new(project).map_err(PlatformAuditConfigError::Project)?,
            writer: WriterId::new(format!("{AUDIT_WRITER_ID_PREFIX}{instance}"))
                .map_err(PlatformAuditConfigError::Instance)?,
        })
    }
}

fn required<E: ReadEnv>(env: &E, name: &'static str) -> Result<String, PlatformAuditConfigError> {
    env.var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or(PlatformAuditConfigError::Missing(name))
}

#[cfg(test)]
mod tests;
