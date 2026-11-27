use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_protocol::{AuthorityResourceId, AuthorityTaskScope};
use uuid::Uuid;

use crate::handle_store::{Entry, Store};
use crate::{CleanupHandle, HandleStoreError};

/// The native handles of the leases a worker holds.
///
/// A handle leaves the store only through [`take_for_cleanup`](Self::take_for_cleanup). The entry
/// stays until the returned [`CleanupHandle`] is dropped, and only then does the lease's
/// authority resource report completion.
pub(crate) struct RetainedHandles<T>(Arc<Store<T>>);

impl<T> Default for RetainedHandles<T> {
    fn default() -> Self {
        Self(Arc::new(Store {
            entries: Mutex::new(HashMap::new()),
            scope: AuthorityTaskScope::default(),
        }))
    }
}

impl<T> RetainedHandles<T> {
    pub(crate) fn insert(&self, lease: &LeaseId, handle: T) -> Result<(), HandleStoreError> {
        let mut entries = self.0.entries();
        if entries.contains_key(lease) {
            return Err(HandleStoreError::Retained);
        }
        let (resource, completion) = self
            .0
            .scope
            .track(lease.clone(), format!("hyperlight-{}", Uuid::new_v4()))?;
        entries.insert(
            lease.clone(),
            Entry {
                resource,
                completion,
                handle: Some(Arc::new(Mutex::new(handle))),
                retiring: false,
            },
        );
        Ok(())
    }

    pub(crate) fn len(&self) -> usize {
        self.0.entries().len()
    }

    pub(crate) fn contains(&self, lease: &LeaseId) -> bool {
        self.0.entries().contains_key(lease)
    }

    pub(crate) fn get(&self, lease: &LeaseId) -> Option<Arc<Mutex<T>>> {
        self.0
            .entries()
            .get(lease)
            .filter(|entry| !entry.retiring)
            .and_then(|entry| entry.handle.clone())
    }

    pub(crate) fn take_for_cleanup(&self, lease: &LeaseId) -> Result<Option<CleanupHandle<T>>, HandleStoreError> {
        let mut entries = self.0.entries();
        let Some(entry) = entries.get_mut(lease) else {
            return Ok(None);
        };
        entry.retiring = true;
        let handle = entry.handle.take().ok_or(HandleStoreError::CleanupUnconfirmed)?;
        match Arc::try_unwrap(handle) {
            Ok(value) => Ok(Some(CleanupHandle::new(
                value.into_inner().unwrap_or_else(PoisonError::into_inner),
                Arc::clone(&self.0),
                lease.clone(),
            ))),
            Err(handle) => {
                entry.handle = Some(handle);
                Err(HandleStoreError::Borrowed)
            }
        }
    }

    pub(crate) fn inventory(&self) -> Vec<(LeaseId, AuthorityResourceId)> {
        self.0
            .entries()
            .iter()
            .map(|(lease, entry)| (lease.clone(), entry.resource.clone()))
            .collect()
    }

    pub(crate) fn matches(&self, lease: &LeaseId, resource: &AuthorityResourceId) -> bool {
        self.0
            .entries()
            .get(lease)
            .is_some_and(|entry| &entry.resource == resource)
    }

    pub(crate) fn is_completed(&self, lease: &LeaseId, resource: &AuthorityResourceId) -> bool {
        self.0.scope.is_completed(lease, resource)
    }
}

#[cfg(test)]
mod tests;
