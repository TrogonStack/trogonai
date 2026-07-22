use std::collections::{HashSet, VecDeque};
use std::sync::Mutex;

use trogon_std::env::ReadEnv;

use crate::constants::{DEFAULT_PUSH_DLQ_DEDUP_LRU_SIZE, ENV_PUSH_DLQ_DEDUP_LRU_SIZE};
use crate::push::push_idempotency_key::PushIdempotencyKey;

#[derive(Debug)]
pub struct PushDlqDedupGate {
    capacity: usize,
    state: Mutex<(VecDeque<String>, HashSet<String>)>,
}

impl PushDlqDedupGate {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            state: Mutex::new((VecDeque::new(), HashSet::new())),
        }
    }

    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        let capacity = env
            .var(ENV_PUSH_DLQ_DEDUP_LRU_SIZE)
            .ok()
            .and_then(|raw| raw.trim().parse::<usize>().ok())
            .unwrap_or(DEFAULT_PUSH_DLQ_DEDUP_LRU_SIZE);
        Self::with_capacity(capacity)
    }

    /// Returns `true` when the publish should proceed; `false` when the key was seen recently.
    pub fn try_acquire(&self, key: &PushIdempotencyKey) -> bool {
        let key = key.as_str().to_owned();
        // Recover from a poisoned lock — partial inserts can't leave the
        // (deque, set) pair logically corrupt, and panicking every reader
        // after a single unrelated panic would just turn dedup into a DoS.
        let mut guard = match self.state.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let (order, seen) = &mut *guard;

        if seen.contains(&key) {
            return false;
        }

        while seen.len() >= self.capacity {
            match order.pop_front() {
                Some(evicted) => {
                    seen.remove(&evicted);
                }
                // `seen` is full but `order` is empty — the two have drifted
                // (e.g. across a panic/poison recovery). Drop the orphaned set
                // entries so the LRU stays bounded instead of growing past
                // `capacity` forever.
                None => {
                    seen.clear();
                    break;
                }
            }
        }

        seen.insert(key.clone());
        order.push_back(key);
        true
    }

    /// Release a previously-acquired key so a subsequent `try_acquire` for
    /// the same key returns `true` again. Use after a publish FAILED so a
    /// JetStream redelivery isn't silently dedup-suppressed.
    ///
    /// Returns `true` when the key was present (and was removed), `false`
    /// when it had already been evicted or never acquired.
    pub fn release(&self, key: &PushIdempotencyKey) -> bool {
        let key = key.as_str().to_owned();
        let mut guard = match self.state.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let (order, seen) = &mut *guard;
        if !seen.remove(&key) {
            return false;
        }
        // Also drop from the FIFO so capacity accounting stays accurate.
        if let Some(pos) = order.iter().position(|k| k == &key) {
            order.remove(pos);
        }
        true
    }
}

impl Default for PushDlqDedupGate {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_PUSH_DLQ_DEDUP_LRU_SIZE)
    }
}

#[cfg(test)]
mod tests;
