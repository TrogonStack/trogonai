use std::sync::{Mutex, MutexGuard, PoisonError};

/// Locks a mutex whose protected state stays consistent even when a holder panicked.
pub(crate) trait MutexExt<T> {
    fn lock_clean(&self) -> MutexGuard<'_, T>;
}

impl<T> MutexExt<T> for Mutex<T> {
    fn lock_clean(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
