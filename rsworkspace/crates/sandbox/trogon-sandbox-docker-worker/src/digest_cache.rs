//! Process-memory LRU of recently pulled snapshot digests for broker locality scoring.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, PoisonError};

use trogon_sandbox_contract::Digest;

pub struct DigestCache {
    capacity: usize,
    order: Mutex<VecDeque<Digest>>,
}

impl DigestCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            order: Mutex::new(VecDeque::new()),
        }
    }

    fn locked(&self) -> MutexGuard<'_, VecDeque<Digest>> {
        self.order.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn touch(&self, digest: Digest) {
        let mut order = self.locked();
        order.retain(|d| d != &digest);
        order.push_front(digest);
        while order.len() > self.capacity {
            order.pop_back();
        }
    }

    pub fn contains(&self, digest: &Digest) -> bool {
        self.locked().iter().any(|d| d == digest)
    }

    pub fn digests(&self) -> Vec<Digest> {
        self.locked().iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests;
