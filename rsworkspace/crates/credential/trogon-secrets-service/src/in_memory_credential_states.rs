use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use trogon_credential_domain::CredentialKey;
use trogonai_proto::gateway::credentials::state_v1::CredentialStateSnapshot;

use super::{CredentialStateRead, CredentialStateReadError};

/// Credential states held in memory, for tests that drive the release core
/// without a credential stream.
#[derive(Clone, Default)]
pub struct InMemoryCredentialStates {
    states: Arc<Mutex<BTreeMap<CredentialKey, CredentialStateSnapshot>>>,
    failure: Arc<Mutex<Option<CredentialStateReadError>>>,
    reads: Arc<AtomicUsize>,
}

impl InMemoryCredentialStates {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the state `credential` folds to.
    pub fn set(&self, credential: CredentialKey, state: CredentialStateSnapshot) {
        self.states
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(credential, state);
    }

    /// Makes every later read fail with `error`.
    pub fn fail_with(&self, error: CredentialStateReadError) {
        *self.failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(error);
    }

    /// How many reads were asked for, failed ones included.
    pub fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
}

impl CredentialStateRead for InMemoryCredentialStates {
    async fn read(
        &self,
        credential: &CredentialKey,
    ) -> Result<Option<CredentialStateSnapshot>, CredentialStateReadError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = self.failure.lock().unwrap_or_else(PoisonError::into_inner).clone() {
            return Err(error);
        }
        Ok(self
            .states
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(credential)
            .cloned())
    }
}

#[cfg(test)]
mod tests;
