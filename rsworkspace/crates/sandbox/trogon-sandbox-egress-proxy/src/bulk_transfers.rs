//! Keeps the relay budget's bulk transfers off the CPU while the tunnel-ends
//! cases run against their deadlines. Those suites run as separate test
//! files, so neither competes with the other for time.

use std::future::Future;

use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use tokio::time::timeout;

mod constants;
pub(crate) use constants::STEP;
use constants::{BULK, LOCK};

/// A wait that outlasted its bound: the step it names, or the lock a case
/// queued on.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct BoundExceededError(String);

/// `step`, or an error naming `what` once [`STEP`] has passed, so that a case
/// never waits on something that will not happen.
pub(crate) async fn bounded<T>(what: &str, step: impl Future<Output = T>) -> Result<T, BoundExceededError> {
    timeout(STEP, step)
        .await
        .map_err(|_| BoundExceededError(format!("{what} did not finish within {STEP:?}")))
}

/// Held by a case whose deadlines assume it is not sharing the CPU with a
/// bulk transfer; such cases still run alongside each other.
pub(crate) async fn timed() -> Result<RwLockReadGuard<'static, ()>, BoundExceededError> {
    timed_on(&BULK).await
}

/// Held by a bulk transfer for as long as it runs.
pub(crate) async fn bulk() -> Result<RwLockWriteGuard<'static, ()>, BoundExceededError> {
    bulk_on(&BULK).await
}

async fn timed_on(lock: &RwLock<()>) -> Result<RwLockReadGuard<'_, ()>, BoundExceededError> {
    timeout(LOCK, lock.read())
        .await
        .map_err(|_| BoundExceededError(format!("a bulk transfer held the CPU for over {LOCK:?}")))
}

async fn bulk_on(lock: &RwLock<()>) -> Result<RwLockWriteGuard<'_, ()>, BoundExceededError> {
    timeout(LOCK, lock.write()).await.map_err(|_| {
        BoundExceededError(format!(
            "another bulk transfer or a timed case held the CPU for over {LOCK:?}"
        ))
    })
}

#[cfg(test)]
mod tests;
