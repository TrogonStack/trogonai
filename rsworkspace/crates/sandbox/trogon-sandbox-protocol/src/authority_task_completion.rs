use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use trogon_sandbox_contract::LeaseId;

#[derive(Debug)]
pub(crate) struct AuthorityTaskState {
    pub(crate) scope: Arc<()>,
    pub(crate) lease: LeaseId,
    pub(crate) resource: String,
    pub(crate) completed: AtomicBool,
}

/// The handle that confirms a tracked task finished. Two handles are equal only when they are the same handle.
#[derive(Debug, Clone)]
pub struct AuthorityTaskCompletion(pub(crate) Arc<AuthorityTaskState>);

impl PartialEq for AuthorityTaskCompletion {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for AuthorityTaskCompletion {}

impl Hash for AuthorityTaskCompletion {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state);
    }
}

impl AuthorityTaskCompletion {
    /// Records that the task finished.
    pub fn confirm(&self) {
        self.0.completed.store(true, Ordering::Release);
    }
}
