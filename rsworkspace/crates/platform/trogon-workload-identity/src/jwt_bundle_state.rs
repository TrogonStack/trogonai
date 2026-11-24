use std::sync::{Arc, Mutex};

use tokio::time::Instant;

use crate::constants::JWT_BUNDLE_MAX_STALENESS;
use crate::{JwtBundleSet, JwtBundleSourceError};

/// What the Workload API stream last delivered and whether it is still live.
#[derive(Default)]
pub(crate) struct JwtBundleState {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    current: Option<Arc<JwtBundleSet>>,
    lost_at: Option<Instant>,
}

impl JwtBundleState {
    pub(crate) fn current(&self) -> Result<Arc<JwtBundleSet>, JwtBundleSourceError> {
        let inner = self.inner.lock().map_err(|_| JwtBundleSourceError::Stale)?;
        let current = inner.current.as_ref().ok_or(JwtBundleSourceError::NeverDelivered)?;
        if inner
            .lost_at
            .is_some_and(|lost_at| lost_at.elapsed() > JWT_BUNDLE_MAX_STALENESS)
        {
            return Err(JwtBundleSourceError::Stale);
        }
        Ok(Arc::clone(current))
    }

    /// An accepted update replaces the whole set and marks the stream live.
    pub(crate) fn replace(&self, bundles: JwtBundleSet) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.current = Some(Arc::new(bundles));
            inner.lost_at = None;
        }
    }

    /// The stream ended. The first loss starts the staleness clock; further
    /// failed attempts do not restart it.
    pub(crate) fn lost(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.lost_at.get_or_insert_with(Instant::now);
        }
    }
}

#[cfg(test)]
mod tests;
