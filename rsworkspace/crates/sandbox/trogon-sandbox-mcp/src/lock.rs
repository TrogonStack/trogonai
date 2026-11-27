use std::sync::{Mutex, MutexGuard, PoisonError};

/// Locks `mutex`, recovering the guard when a panicking holder poisoned it. Every value guarded in this crate
/// stays consistent between statements, so a poisoned lock is still safe to use.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
