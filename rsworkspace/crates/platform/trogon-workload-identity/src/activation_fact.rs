use std::num::NonZeroU64;
use std::time::SystemTime;

use crate::RegistrationBinding;

#[derive(Debug, Clone)]
pub struct ActivationFact {
    pub(crate) binding: RegistrationBinding,
    pub(crate) recovery_fence: NonZeroU64,
    pub(crate) registration_revision: NonZeroU64,
    pub(crate) trust_revision: NonZeroU64,
    pub(crate) source_revision: NonZeroU64,
    pub(crate) activation_revision: NonZeroU64,
    pub(crate) leaf_sha256: [u8; 32],
    pub(crate) public_key_sha256: [u8; 32],
    pub(crate) bundle_sha256: [u8; 32],
    pub(crate) not_before: SystemTime,
    pub(crate) expires_at: SystemTime,
}

impl ActivationFact {
    pub fn binding(&self) -> &RegistrationBinding {
        &self.binding
    }
    pub fn recovery_fence(&self) -> NonZeroU64 {
        self.recovery_fence
    }
    pub fn registration_revision(&self) -> NonZeroU64 {
        self.registration_revision
    }
    pub fn trust_revision(&self) -> NonZeroU64 {
        self.trust_revision
    }
    pub fn source_revision(&self) -> u64 {
        self.source_revision.get()
    }
    pub fn activation_revision(&self) -> NonZeroU64 {
        self.activation_revision
    }
    pub fn leaf_sha256(&self) -> &[u8; 32] {
        &self.leaf_sha256
    }
    pub fn public_key_sha256(&self) -> &[u8; 32] {
        &self.public_key_sha256
    }
    pub fn bundle_sha256(&self) -> &[u8; 32] {
        &self.bundle_sha256
    }
    pub fn not_before(&self) -> SystemTime {
        self.not_before
    }
    pub fn expires_at(&self) -> SystemTime {
        self.expires_at
    }
}
