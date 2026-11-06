use std::num::NonZeroU64;
use std::time::{Duration, SystemTime};

use buffa::Message;
use trogon_workload_identity::constants::{MAX_BINDINGS, MAX_REGISTRATION_LIFETIME, MAX_ROOTS};
use trogon_workload_identity::{RegistrationBinding, WorkloadRole};
use trogonai_proto::platform::workload_identity::v1alpha1::{RegistrationSnapshot, WorkloadRole as WireRole};

use crate::{
    ApprovedRegistration, AuthorityVersion, CanonicalAgentId, MAX_SNAPSHOT_BYTES, ProducerConfig, RegistrationError,
    Sha256Digest,
};

#[derive(Debug)]
pub(crate) struct RegistrationHead {
    version: AuthorityVersion,
    issued_at: SystemTime,
    roots: Vec<Sha256Digest>,
}

impl RegistrationHead {
    pub(crate) fn decode(config: &ProducerConfig, payload: &[u8]) -> Result<Self, RegistrationError> {
        if payload.len() > MAX_SNAPSHOT_BYTES {
            return Err(RegistrationError::Bounds);
        }
        let wire = RegistrationSnapshot::decode_from_slice(payload).map_err(RegistrationError::Decode)?;
        if wire.encode_to_vec() != payload {
            return Err(RegistrationError::NoncanonicalPayload);
        }
        if wire.tenant != config.scope().tenant().as_str()
            || wire.project != config.scope().project().as_str()
            || wire.recovery_fence != config.recovery_fence().get()
        {
            return Err(RegistrationError::Scope);
        }
        let version = AuthorityVersion::new(
            config.recovery_fence(),
            NonZeroU64::new(wire.registration_revision).ok_or(RegistrationError::Revision)?,
            NonZeroU64::new(wire.trust_revision).ok_or(RegistrationError::Revision)?,
        );
        fn timestamp(
            value: Option<&buffa_types::google::protobuf::Timestamp>,
        ) -> Result<SystemTime, RegistrationError> {
            let value = value.ok_or(RegistrationError::Lease)?;
            if value.seconds <= 0 || !(0..1_000_000_000).contains(&value.nanos) {
                return Err(RegistrationError::Lease);
            }
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::new(value.seconds as u64, value.nanos as u32))
                .ok_or(RegistrationError::Lease)
        }
        let issued_at = timestamp(wire.issued_at.as_option())?;
        let expires_at = timestamp(wire.expires_at.as_option())?;
        let lifetime = expires_at
            .duration_since(issued_at)
            .map_err(|_| RegistrationError::Lease)?;
        if lifetime.is_zero()
            || lifetime > MAX_REGISTRATION_LIFETIME
            || wire.bindings.len() > MAX_BINDINGS
            || wire.approved_root_sha256.len() > MAX_ROOTS
        {
            return Err(RegistrationError::Bounds);
        }
        let mut previous = None;
        for binding in &wire.bindings {
            let role = match binding.role.as_known() {
                Some(WireRole::EgressProxy) => WorkloadRole::EgressProxy,
                Some(WireRole::Agent) => WorkloadRole::Agent,
                Some(WireRole::Supervisor) => WorkloadRole::Supervisor,
                _ => return Err(RegistrationError::Scope),
            };
            if role == WorkloadRole::Agent {
                CanonicalAgentId::new(
                    binding
                        .application_id
                        .as_deref()
                        .ok_or(RegistrationError::ApplicationIdentity)?,
                )?;
            }
            let checked = RegistrationBinding::new(
                &binding.spiffe_id,
                &binding.tenant,
                &binding.project,
                role,
                binding.application_id.as_deref(),
            )
            .map_err(RegistrationError::Binding)?;
            if checked.tenant() != config.scope().tenant().as_str()
                || checked.project() != config.scope().project().as_str()
                || checked.spiffe_id().trust_domain() != config.scope().trust_domain()
                || previous.as_ref().is_some_and(|id| id >= checked.spiffe_id())
            {
                return Err(RegistrationError::Scope);
            }
            previous = Some(checked.spiffe_id().clone());
        }
        let roots = wire
            .approved_root_sha256
            .iter()
            .map(|root| Sha256Digest::from_bytes(root))
            .collect::<Result<Vec<_>, _>>()?;
        if roots.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(RegistrationError::Bounds);
        }
        Ok(Self {
            version,
            issued_at,
            roots,
        })
    }

    pub(crate) fn check_successor(&self, next: &ApprovedRegistration) -> Result<(), RegistrationError> {
        next.version().check_successor(self.version)?;
        if next.lease().issued_at() <= self.issued_at
            || (next.roots() != self.roots && next.version().trust_revision() == self.version.trust_revision())
        {
            return Err(RegistrationError::Revision);
        }
        next.check_current()
    }
}
