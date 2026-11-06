use std::num::NonZeroU64;
use std::time::{Duration, Instant, SystemTime};

use buffa::{EnumValue, Message, MessageField};
use buffa_types::google::protobuf::Timestamp;
use bytes::Bytes;
use ed25519_dalek::Signature;
use trogon_workload_identity::constants::{MAX_BINDINGS, MAX_ROOTS};
use trogon_workload_identity::{RegistrationBinding, WorkloadRole};
use trogonai_proto::platform::workload_identity::v1alpha1::{
    RegistrationBinding as WireBinding, RegistrationSnapshot, WorkloadRole as WireRole,
};

use crate::{
    ApprovalLease, AuthorityVersion, MAX_SNAPSHOT_BYTES, ProducerConfig, RegistrationError, SIGNING_CONTEXT,
    Sha256Digest,
};

#[derive(Debug, Clone)]
pub struct ApprovedRegistration {
    config: ProducerConfig,
    version: AuthorityVersion,
    lease: ApprovalLease,
    bindings: Vec<RegistrationBinding>,
    roots: Vec<Sha256Digest>,
}

impl ApprovedRegistration {
    pub fn verify(config: &ProducerConfig, payload: &[u8], signature: &[u8; 64]) -> Result<Self, RegistrationError> {
        let started = Instant::now();
        let now = SystemTime::now();
        if payload.len() > MAX_SNAPSHOT_BYTES {
            return Err(RegistrationError::Bounds);
        }
        config
            .approval_key()
            .verifying_key()
            .verify_strict(&[SIGNING_CONTEXT, payload].concat(), &Signature::from_bytes(signature))
            .map_err(RegistrationError::Signature)?;
        let message = RegistrationSnapshot::decode_from_slice(payload).map_err(RegistrationError::Decode)?;
        if message.tenant != config.scope().tenant().as_str()
            || message.project != config.scope().project().as_str()
            || message.recovery_fence != config.recovery_fence().get()
        {
            return Err(RegistrationError::Scope);
        }
        let version = AuthorityVersion::new(
            config.recovery_fence(),
            NonZeroU64::new(message.registration_revision).ok_or(RegistrationError::Revision)?,
            NonZeroU64::new(message.trust_revision).ok_or(RegistrationError::Revision)?,
        );
        let lease = ApprovalLease::new(
            timestamp(message.issued_at.as_option())?,
            timestamp(message.expires_at.as_option())?,
            started,
            now,
        )?;
        if message.bindings.len() > MAX_BINDINGS || message.approved_root_sha256.len() > MAX_ROOTS {
            return Err(RegistrationError::Bounds);
        }
        let mut bindings = Vec::with_capacity(message.bindings.len());
        for binding in message.bindings {
            let role = match binding.role.as_known() {
                Some(WireRole::WORKLOAD_ROLE_EGRESS_PROXY) => WorkloadRole::EgressProxy,
                Some(WireRole::WORKLOAD_ROLE_AGENT) => WorkloadRole::Agent,
                Some(WireRole::WORKLOAD_ROLE_SUPERVISOR) => WorkloadRole::Supervisor,
                _ => return Err(RegistrationError::Scope),
            };
            if role == WorkloadRole::Agent {
                crate::CanonicalAgentId::new(
                    binding
                        .application_id
                        .as_deref()
                        .ok_or(RegistrationError::ApplicationIdentity)?,
                )?;
            }
            let binding = RegistrationBinding::new(
                &binding.spiffe_id,
                &binding.tenant,
                &binding.project,
                role,
                binding.application_id.as_deref(),
            )
            .map_err(RegistrationError::Binding)?;
            if binding.tenant() != config.scope().tenant().as_str()
                || binding.project() != config.scope().project().as_str()
                || binding.spiffe_id().trust_domain() != config.scope().trust_domain()
            {
                return Err(RegistrationError::Scope);
            }
            bindings.push(binding);
        }
        bindings.sort_by(|a, b| a.spiffe_id().cmp(b.spiffe_id()));
        if bindings
            .windows(2)
            .any(|pair| pair[0].spiffe_id() == pair[1].spiffe_id())
        {
            return Err(RegistrationError::Bounds);
        }
        let mut roots = message
            .approved_root_sha256
            .iter()
            .map(|bytes| Sha256Digest::from_bytes(bytes))
            .collect::<Result<Vec<_>, _>>()?;
        roots.sort_unstable();
        if roots.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(RegistrationError::Bounds);
        }
        let approved = Self {
            config: config.clone(),
            version,
            lease,
            bindings,
            roots,
        };
        if approved.payload().as_ref() != payload {
            return Err(RegistrationError::NoncanonicalPayload);
        }
        approved.check_current()?;
        Ok(approved)
    }

    pub fn config(&self) -> &ProducerConfig {
        &self.config
    }
    pub fn version(&self) -> AuthorityVersion {
        self.version
    }
    pub fn lease(&self) -> &ApprovalLease {
        &self.lease
    }
    pub fn bindings(&self) -> &[RegistrationBinding] {
        &self.bindings
    }
    pub fn roots(&self) -> &[Sha256Digest] {
        &self.roots
    }
    pub fn check_current(&self) -> Result<(), RegistrationError> {
        self.lease.check_current()
    }

    pub fn payload(&self) -> Bytes {
        RegistrationSnapshot {
            tenant: self.config.scope().tenant().as_str().into(),
            project: self.config.scope().project().as_str().into(),
            recovery_fence: self.version.recovery_fence().get(),
            registration_revision: self.version.registration_revision().get(),
            trust_revision: self.version.trust_revision().get(),
            issued_at: MessageField::some(Timestamp::from(self.lease.issued_at())),
            expires_at: MessageField::some(Timestamp::from(self.lease.expires_at())),
            bindings: self
                .bindings
                .iter()
                .map(|binding| WireBinding {
                    spiffe_id: binding.spiffe_id().to_string(),
                    tenant: binding.tenant().into(),
                    project: binding.project().into(),
                    role: EnumValue::from(match binding.role() {
                        WorkloadRole::EgressProxy => WireRole::WORKLOAD_ROLE_EGRESS_PROXY,
                        WorkloadRole::Agent => WireRole::WORKLOAD_ROLE_AGENT,
                        WorkloadRole::Supervisor => WireRole::WORKLOAD_ROLE_SUPERVISOR,
                    }),
                    application_id: binding.application_id().map(|identity| identity.as_str().to_owned()),
                })
                .collect(),
            approved_root_sha256: self.roots.iter().map(|root| root.as_bytes().to_vec()).collect(),
        }
        .encode_to_vec()
        .into()
    }

    pub fn digest(&self) -> Sha256Digest {
        Sha256Digest::hash(&self.payload())
    }

    pub fn check_successor(&self, previous: &Self) -> Result<(), RegistrationError> {
        if self.config != previous.config || self.lease.issued_at() <= previous.lease.issued_at() {
            return Err(RegistrationError::Revision);
        }
        self.version.check_successor(previous.version)?;
        if self.roots != previous.roots && self.version.trust_revision() == previous.version.trust_revision() {
            return Err(RegistrationError::Revision);
        }
        self.check_current()
    }
}

fn timestamp(value: Option<&Timestamp>) -> Result<SystemTime, RegistrationError> {
    let value = value.ok_or(RegistrationError::Lease)?;
    if value.seconds <= 0 || !(0..1_000_000_000).contains(&value.nanos) {
        return Err(RegistrationError::Lease);
    }
    SystemTime::UNIX_EPOCH
        .checked_add(Duration::new(value.seconds as u64, value.nanos as u32))
        .ok_or(RegistrationError::Lease)
}
