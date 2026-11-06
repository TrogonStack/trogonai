use std::num::NonZeroU64;
use std::time::SystemTime;

use crate::constants::{MAX_BINDINGS, MAX_ROOTS};
use crate::{RegistrationBinding, RegistrationError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationSet {
    recovery_fence: NonZeroU64,
    registration_revision: NonZeroU64,
    trust_revision: NonZeroU64,
    expires_at: SystemTime,
    bindings: Vec<RegistrationBinding>,
    approved_roots: Vec<[u8; 32]>,
}

impl RegistrationSet {
    pub fn new(
        recovery_fence: NonZeroU64,
        registration_revision: NonZeroU64,
        trust_revision: NonZeroU64,
        expires_at: SystemTime,
        mut bindings: Vec<RegistrationBinding>,
        mut approved_roots: Vec<[u8; 32]>,
    ) -> Result<Self, RegistrationError> {
        if bindings.len() > MAX_BINDINGS || approved_roots.is_empty() || approved_roots.len() > MAX_ROOTS {
            return Err(RegistrationError::Bounds);
        }
        bindings.sort_by(|a, b| a.spiffe_id().cmp(b.spiffe_id()));
        if bindings
            .windows(2)
            .any(|pair| pair[0].spiffe_id() == pair[1].spiffe_id())
        {
            return Err(RegistrationError::Binding);
        }
        approved_roots.sort_unstable();
        if approved_roots.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(RegistrationError::Bounds);
        }
        Ok(Self {
            recovery_fence,
            registration_revision,
            trust_revision,
            expires_at,
            bindings,
            approved_roots,
        })
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
    pub fn expires_at(&self) -> SystemTime {
        self.expires_at
    }
    pub fn bindings(&self) -> &[RegistrationBinding] {
        &self.bindings
    }
    pub fn approved_roots(&self) -> &[[u8; 32]] {
        &self.approved_roots
    }
}
