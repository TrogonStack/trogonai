use std::sync::Arc;

use trogon_sandbox_contract::LeaseId;

use crate::handle_store::Store;

/// A native handle taken out of the store for disposal.
///
/// The lease's entry leaves the store, and its authority resource reports completion, only when
/// this value is dropped, after the handle itself was released.
pub(crate) struct CleanupHandle<T> {
    value: Option<T>,
    store: Arc<Store<T>>,
    lease: LeaseId,
}

impl<T> CleanupHandle<T> {
    pub(crate) fn new(value: T, store: Arc<Store<T>>, lease: LeaseId) -> Self {
        Self {
            value: Some(value),
            store,
            lease,
        }
    }

    /// Hands the handle to `release` and then confirms the cleanup.
    pub(crate) fn dispose(mut self, release: impl FnOnce(T)) {
        if let Some(value) = self.value.take() {
            release(value);
        }
    }
}

impl<T> Drop for CleanupHandle<T> {
    fn drop(&mut self) {
        drop(self.value.take());
        if let Some(entry) = self.store.entries().remove(&self.lease) {
            entry.confirm();
        }
    }
}
