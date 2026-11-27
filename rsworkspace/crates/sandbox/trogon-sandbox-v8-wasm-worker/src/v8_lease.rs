use std::sync::Arc;

use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;
use trogon_sandbox_protocol::{AuthorityResourceId, AuthorityTaskCompletion};

use crate::JsSession;

/// A lease the worker is running or retaining.
pub(crate) struct V8Lease {
    pub(crate) resource: AuthorityResourceId,
    pub(crate) completion: AuthorityTaskCompletion,
    pub(crate) cancel: CancellationToken,
    pub(crate) finished: watch::Receiver<bool>,
    pub(crate) session: Mutex<Option<Arc<JsSession>>>,
    pub(crate) persistent: bool,
}
