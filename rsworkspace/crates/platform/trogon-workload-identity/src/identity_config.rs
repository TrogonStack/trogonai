use std::num::NonZeroU64;

use crate::{RegistrationBinding, UnixEndpoint};

#[derive(Debug, Clone)]
pub struct IdentityConfig {
    pub(crate) endpoint: UnixEndpoint,
    pub(crate) local: RegistrationBinding,
    pub(crate) recovery_fence: NonZeroU64,
}

impl IdentityConfig {
    pub fn new(endpoint: UnixEndpoint, local: RegistrationBinding, recovery_fence: NonZeroU64) -> Self {
        Self { endpoint, local, recovery_fence }
    }
    pub fn endpoint(&self) -> &UnixEndpoint { &self.endpoint }
    pub fn local(&self) -> &RegistrationBinding { &self.local }
    pub fn recovery_fence(&self) -> NonZeroU64 { self.recovery_fence }
}
