use std::sync::{Arc, Mutex};

use super::Sent;
use crate::lock::lock;

/// A shared, inspectable list of the requests a bus received.
#[derive(Debug, Clone, Default)]
pub(crate) struct SentLog(Arc<Mutex<Vec<Sent>>>);

impl SentLog {
    pub(crate) fn all(&self) -> Vec<Sent> {
        lock(&self.0).clone()
    }

    pub(crate) fn push(&self, sent: Sent) {
        lock(&self.0).push(sent);
    }
}
