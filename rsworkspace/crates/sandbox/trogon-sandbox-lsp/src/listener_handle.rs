use std::sync::Arc;

use crate::inner::Inner;
use crate::listener_kind::ListenerKind;
use crate::lock_ext::LockExt;

/// Removes a listener when [`ListenerHandle::detach`] is called. Dropping the handle leaves the
/// listener attached.
pub struct ListenerHandle {
    inner: Arc<Inner>,
    id: u64,
    kind: ListenerKind,
}

impl ListenerHandle {
    pub(crate) fn new(inner: Arc<Inner>, id: u64, kind: ListenerKind) -> Self {
        Self { inner, id, kind }
    }

    pub fn detach(self) {
        match self.kind {
            ListenerKind::Stderr => self.inner.stderr_handlers.locked().retain(|(id, _)| *id != self.id),
            ListenerKind::Notification => {
                let attached = {
                    let mut listeners = self.inner.listeners.locked();
                    listeners
                        .iter()
                        .position(|l| l.id == self.id)
                        .and_then(|index| listeners.remove(index).attached)
                };
                if let Some(handle) = attached {
                    handle.detach();
                }
            }
        }
    }
}
