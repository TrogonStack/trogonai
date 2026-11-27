use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Mutex;

use trogon_sandbox_contract::LeaseId;

use crate::SshSessionId;
use crate::reserved::Reserved;
use crate::session_refusal::SessionRefusalError;

/// Session id to lease binding, with the concurrent-session cap.
pub(crate) struct SessionTable<T> {
    max: NonZeroUsize,
    entries: Mutex<HashMap<SshSessionId, (LeaseId, T)>>,
}

impl<T: Clone> SessionTable<T> {
    pub(crate) fn new(max: NonZeroUsize) -> Self {
        Self {
            max,
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn reserve(
        &self,
        session_id: &SshSessionId,
        lease: &LeaseId,
        value: impl FnOnce() -> T,
    ) -> Result<Reserved, SessionRefusalError> {
        let mut entries = self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((bound, _)) = entries.get(session_id) {
            return if bound == lease {
                Ok(Reserved::Existing)
            } else {
                Err(SessionRefusalError::BoundToAnotherLease)
            };
        }
        if entries.len() >= self.max.get() {
            return Err(SessionRefusalError::AtCapacity { max: self.max });
        }
        entries.insert(session_id.clone(), (lease.clone(), value()));
        Ok(Reserved::New)
    }

    pub(crate) fn get(&self, session_id: &SshSessionId) -> Option<(LeaseId, T)> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(session_id)
            .cloned()
    }

    pub(crate) fn remove(&self, session_id: &SshSessionId) {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(session_id);
    }

    pub(crate) fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}
