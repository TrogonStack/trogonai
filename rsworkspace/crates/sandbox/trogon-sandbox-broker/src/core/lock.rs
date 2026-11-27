use std::sync::{Mutex, MutexGuard, PoisonError};

/// Locks `mutex`, recovering the guard when a panicking holder poisoned it.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
