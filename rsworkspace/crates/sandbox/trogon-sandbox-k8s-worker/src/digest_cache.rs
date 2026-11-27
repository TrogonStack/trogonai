use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, PoisonError};

use trogon_sandbox_contract::Digest;

/// Process-memory LRU of snapshot digests available on this worker host.
#[derive(Debug)]
pub struct DigestCache {
    max_entries: usize,
    inner: Mutex<VecDeque<Digest>>,
}

impl DigestCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            max_entries: max_entries.max(1),
            inner: Mutex::new(VecDeque::new()),
        }
    }

    pub fn insert(&self, digest: Digest) {
        let mut cache = self.lock();
        cache.retain(|d| d != &digest);
        cache.push_back(digest);
        while cache.len() > self.max_entries {
            cache.pop_front();
        }
    }

    pub fn contains(&self, digest: &Digest) -> bool {
        self.lock().iter().any(|d| d == digest)
    }

    pub fn list(&self) -> Vec<Digest> {
        self.lock().iter().cloned().collect()
    }
}

impl DigestCache {
    fn lock(&self) -> MutexGuard<'_, VecDeque<Digest>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests;
