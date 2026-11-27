use std::sync::{Mutex, MutexGuard, PoisonError};

/// Locks a mutex and keeps going when a panicking holder poisoned it: every guarded value here stays
/// consistent across a panic, because listeners run outside the locks.
pub(crate) trait LockExt<T> {
    fn locked(&self) -> MutexGuard<'_, T>;
}

impl<T> LockExt<T> for Mutex<T> {
    fn locked(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
