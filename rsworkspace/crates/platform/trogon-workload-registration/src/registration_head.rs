use std::num::NonZeroU64;
use std::time::SystemTime;

use buffa::Message;
use trogon_workload_identity::constants::{MAX_BINDINGS, MAX_REGISTRATION_LIFETIME, MAX_ROOTS};
use trogonai_proto::platform::workload_identity::v1alpha1::RegistrationSnapshot;

use crate::registration_timestamp::RegistrationTimestamp;
use crate::scoped_registration_binding::ScopedRegistrationBinding;
use crate::scoped_registration_message::ScopedRegistrationMessage;
use crate::{ApprovedRegistration, AuthorityVersion, ProducerConfig, RegistrationError, Sha256Digest};

#[derive(Debug)]
pub(crate) struct RegistrationHead {
    version: AuthorityVersion,
    issued_at: SystemTime,
    roots: Vec<Sha256Digest>,
}

impl RegistrationHead {
    pub(crate) fn decode(
        config: &ProducerConfig,
        message: &ScopedRegistrationMessage,
    ) -> Result<Self, RegistrationError> {
        let payload = message.payload();
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
        let issued_at = RegistrationTimestamp::decode(wire.issued_at.as_option())?.into_system_time();
        let expires_at = RegistrationTimestamp::decode(wire.expires_at.as_option())?.into_system_time();
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
            let checked = ScopedRegistrationBinding::decode(config, binding)?.into_binding();
            if previous.as_ref().is_some_and(|id| id >= checked.spiffe_id()) {
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
