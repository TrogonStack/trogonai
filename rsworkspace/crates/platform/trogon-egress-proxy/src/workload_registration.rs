use std::num::NonZeroU64;
use std::time::{Duration, SystemTime};

use buffa::Message;
use buffa_types::google::protobuf::Timestamp;
use trogon_workload_identity::constants::{MAX_BINDINGS, MAX_REGISTRATION_LIFETIME, MAX_ROOTS};
use trogon_workload_identity::{IdentityConfig, RegistrationBinding, RegistrationError, RegistrationSet, WorkloadRole};
use trogonai_proto::platform::workload_identity::v1alpha1::{RegistrationSnapshot, WorkloadRole as WireRole};

use crate::constants::REGISTRATION_MAX_BYTES;

#[derive(Debug, thiserror::Error)]
pub enum RegistrationProjectionError {
    #[error("registration projection exceeds its byte bound")]
    TooLarge,
    #[error("registration projection cannot be decoded")]
    Malformed(#[source] buffa::DecodeError),
    #[error("registration projection does not match the configured scope or recovery fence")]
    Scope,
    #[error("registration projection has no current bounded validity")]
    Validity,
    #[error("registration projection is invalid")]
    Registration(#[source] RegistrationError),
}

pub fn decode_registration(
    payload: &[u8],
    config: &IdentityConfig,
    now: SystemTime,
) -> Result<RegistrationSet, RegistrationProjectionError> {
    if payload.len() > REGISTRATION_MAX_BYTES {
        return Err(RegistrationProjectionError::TooLarge);
    }
    let message = RegistrationSnapshot::decode_from_slice(payload).map_err(RegistrationProjectionError::Malformed)?;
    if message.tenant != config.local().tenant()
        || message.project != config.local().project()
        || message.recovery_fence != config.recovery_fence().get()
    {
        return Err(RegistrationProjectionError::Scope);
    }
    let issued_at = system_time(message.issued_at.as_option())?;
    let expires_at = system_time(message.expires_at.as_option())?;
    let lifetime = expires_at
        .duration_since(issued_at)
        .map_err(|_| RegistrationProjectionError::Validity)?;
    if issued_at > now || expires_at <= now || lifetime.is_zero() || lifetime > MAX_REGISTRATION_LIFETIME {
        return Err(RegistrationProjectionError::Validity);
    }
    if message.bindings.len() > MAX_BINDINGS || message.approved_root_sha256.len() > MAX_ROOTS {
        return Err(RegistrationProjectionError::Registration(RegistrationError::Bounds));
    }
    let bindings = message
        .bindings
        .into_iter()
        .map(|binding| {
            let role = match binding.role.as_known() {
                Some(WireRole::WORKLOAD_ROLE_EGRESS_PROXY) => WorkloadRole::EgressProxy,
                Some(WireRole::WORKLOAD_ROLE_AGENT) => WorkloadRole::Agent,
                Some(WireRole::WORKLOAD_ROLE_SUPERVISOR) => WorkloadRole::Supervisor,
                _ => return Err(RegistrationProjectionError::Registration(RegistrationError::Binding)),
            };
            let binding = RegistrationBinding::new(
                &binding.spiffe_id,
                &binding.tenant,
                &binding.project,
                role,
                binding.application_id.as_deref(),
            )
            .map_err(RegistrationProjectionError::Registration)?;
            if binding.tenant() != config.local().tenant()
                || binding.project() != config.local().project()
                || binding.spiffe_id().trust_domain() != config.local().spiffe_id().trust_domain()
            {
                return Err(RegistrationProjectionError::Scope);
            }
            Ok(binding)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let roots = message
        .approved_root_sha256
        .into_iter()
        .map(|root| {
            root.try_into()
                .map_err(|_| RegistrationProjectionError::Registration(RegistrationError::Bounds))
        })
        .collect::<Result<Vec<[u8; 32]>, _>>()?;
    RegistrationSet::new(
        config.recovery_fence(),
        NonZeroU64::new(message.registration_revision)
            .ok_or(RegistrationProjectionError::Registration(RegistrationError::Revision))?,
        NonZeroU64::new(message.trust_revision)
            .ok_or(RegistrationProjectionError::Registration(RegistrationError::Revision))?,
        expires_at,
        bindings,
        roots,
    )
    .map_err(RegistrationProjectionError::Registration)
}

fn system_time(timestamp: Option<&Timestamp>) -> Result<SystemTime, RegistrationProjectionError> {
    let timestamp = timestamp.ok_or(RegistrationProjectionError::Validity)?;
    if timestamp.seconds <= 0 || !(0..1_000_000_000).contains(&timestamp.nanos) {
        return Err(RegistrationProjectionError::Validity);
    }
    SystemTime::UNIX_EPOCH
        .checked_add(Duration::new(timestamp.seconds as u64, timestamp.nanos as u32))
        .ok_or(RegistrationProjectionError::Validity)
}

#[cfg(test)]
mod tests;
