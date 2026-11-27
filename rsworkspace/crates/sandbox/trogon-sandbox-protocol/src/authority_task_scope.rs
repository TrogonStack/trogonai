use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use trogon_sandbox_contract::LeaseId;

use crate::authority_task_completion::AuthorityTaskState;
use crate::{AuthorityResourceId, AuthorityResourceIdError, AuthorityTaskCompletion};

/// The owner of tracked tasks: only completions it issued count as completed for it.
#[derive(Debug, Default)]
pub struct AuthorityTaskScope(Arc<()>);

impl AuthorityTaskScope {
    /// Starts tracking a task on `resource` for `lease`, returning the resource identity to hold and the handle
    /// that confirms the task finished.
    pub fn track(
        &self,
        lease: LeaseId,
        resource: impl Into<String>,
    ) -> Result<(AuthorityResourceId, AuthorityTaskCompletion), AuthorityResourceIdError> {
        let mut resource = AuthorityResourceId::new(resource)?;
        let completion = AuthorityTaskCompletion(Arc::new(AuthorityTaskState {
            scope: self.0.clone(),
            lease,
            resource: resource.value.clone(),
            completed: AtomicBool::new(false),
        }));
        resource.completion = Some(completion.clone());
        Ok((resource, completion))
    }

    /// Whether this scope's task for exactly `lease` and `resource` was confirmed finished.
    pub fn is_completed(&self, lease: &LeaseId, resource: &AuthorityResourceId) -> bool {
        resource.completion.as_ref().is_some_and(|completion| {
            Arc::ptr_eq(&self.0, &completion.0.scope)
                && &completion.0.lease == lease
                && completion.0.resource == resource.value
                && completion.0.completed.load(Ordering::Acquire)
        })
    }
}
