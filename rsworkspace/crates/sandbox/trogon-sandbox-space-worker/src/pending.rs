use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use tokio::sync::watch;

use crate::{SpaceId, SpaceProviderError};

/// A create that has not produced a ready sandbox yet. `cancel` flips the flag and the
/// create path abandons and deletes what it made, so a delete that arrives first always
/// wins.
pub(crate) struct Pending {
    cancelled: AtomicBool,
    sandbox: Mutex<Option<SpaceId>>,
    done: watch::Sender<bool>,
}

impl Pending {
    pub(crate) fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            sandbox: Mutex::new(None),
            done: watch::channel(false).0,
        }
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub(crate) fn set_sandbox(&self, id: SpaceId) {
        *self.sandbox.lock().unwrap_or_else(PoisonError::into_inner) = Some(id);
    }

    pub(crate) fn sandbox(&self) -> Option<SpaceId> {
        self.sandbox.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub(crate) fn ensure_live(&self) -> Result<(), SpaceProviderError> {
        if self.cancelled.load(Ordering::SeqCst) {
            Err(SpaceProviderError::Cancelled)
        } else {
            Ok(())
        }
    }

    pub(crate) fn finish(&self) {
        self.done.send_replace(true);
    }

    pub(crate) fn finished(&self) -> watch::Receiver<bool> {
        self.done.subscribe()
    }
}
