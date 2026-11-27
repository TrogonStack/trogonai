use std::sync::{Mutex, MutexGuard, PoisonError};

/// The contents of `mutex`, even when a panicked holder poisoned it.
pub(crate) fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
