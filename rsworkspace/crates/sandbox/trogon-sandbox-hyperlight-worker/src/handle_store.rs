use std::collections::HashMap;
use std::sync::Arc;
use std::sync::{Mutex, MutexGuard, PoisonError};

use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_protocol::{AuthorityResourceId, AuthorityTaskCompletion, AuthorityTaskScope};

pub(crate) struct Entry<T> {
    pub(crate) resource: AuthorityResourceId,
    pub(crate) completion: AuthorityTaskCompletion,
    pub(crate) handle: Option<Arc<Mutex<T>>>,
    pub(crate) retiring: bool,
}

pub(crate) struct Store<T> {
    pub(crate) entries: Mutex<HashMap<LeaseId, Entry<T>>>,
    pub(crate) scope: AuthorityTaskScope,
}

impl<T> Store<T> {
    pub(crate) fn entries(&self) -> MutexGuard<'_, HashMap<LeaseId, Entry<T>>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<T> Entry<T> {
    pub(crate) fn confirm(&self) {
        self.completion.confirm();
    }
}
