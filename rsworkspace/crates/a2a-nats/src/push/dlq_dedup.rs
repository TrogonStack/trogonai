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
}

impl Default for PushDlqDedupGate {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_PUSH_DLQ_DEDUP_LRU_SIZE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::push::status_transition_id::StatusTransitionId;
    use crate::push::terminal_push_task_state::TerminalPushTaskState;
    use crate::task_id::A2aTaskId;

    fn dlq_key(task: &str, transition: &str, url: &str) -> PushIdempotencyKey {
        let task_id = A2aTaskId::new(task).unwrap();
        let transition_id = StatusTransitionId::new(transition);
        PushIdempotencyKey::derive_dlq(&task_id, &transition_id, url)
    }

    #[test]
    fn second_acquire_with_same_key_is_rejected() {
        let gate = PushDlqDedupGate::with_capacity(8);
        let key = dlq_key("task-1", "failed", "https://example.com/hook");

        assert!(gate.try_acquire(&key));
        assert!(!gate.try_acquire(&key));
    }

    #[test]
    fn different_transition_ids_both_acquire() {
        let gate = PushDlqDedupGate::with_capacity(8);
        let task_id = A2aTaskId::new("task-1").unwrap();
        let url = "https://example.com/hook";

        let failed = PushIdempotencyKey::derive_dlq(
            &task_id,
            &StatusTransitionId::from_terminal(TerminalPushTaskState::Failed),
            url,
        );
        let completed = PushIdempotencyKey::derive_dlq(
            &task_id,
            &StatusTransitionId::from_terminal(TerminalPushTaskState::Completed),
            url,
        );

        assert!(gate.try_acquire(&failed));
        assert!(gate.try_acquire(&completed));
    }

    #[test]
    fn desynced_set_and_order_recover_without_growing_past_capacity() {
        let gate = PushDlqDedupGate::with_capacity(2);
        // Force the desync mid-flight: stuff `seen` past capacity with
        // entries that don't appear in `order`. The next try_acquire must
        // clear `seen` so the gate stays bounded.
        {
            let mut guard = gate.state.lock().unwrap();
            let (_order, seen) = &mut *guard;
            seen.insert("orphan-a".into());
            seen.insert("orphan-b".into());
            seen.insert("orphan-c".into());
        }
        let k = dlq_key("t-new", "failed", "https://new.example/hook");
        assert!(gate.try_acquire(&k));
        let guard = gate.state.lock().unwrap();
        let (order, seen) = &*guard;
        assert_eq!(seen.len(), 1, "orphaned set entries must be dropped");
        assert_eq!(order.len(), 1, "order must reflect the freshly inserted key only");
    }

    #[test]
    fn from_env_reads_configured_capacity() {
        use trogon_std::env::InMemoryEnv;
        let env = InMemoryEnv::new();
        env.set(ENV_PUSH_DLQ_DEDUP_LRU_SIZE, "1");
        let gate = PushDlqDedupGate::from_env(&env);
        let k1 = dlq_key("t1", "failed", "https://a.example/hook");
        let k2 = dlq_key("t2", "failed", "https://b.example/hook");
        assert!(gate.try_acquire(&k1));
        assert!(gate.try_acquire(&k2));
        assert!(gate.try_acquire(&k1), "capacity=1 must evict k1 before k2");
    }

    #[test]
    fn from_env_falls_back_to_default_when_unset_or_invalid() {
        use trogon_std::env::InMemoryEnv;
        let env = InMemoryEnv::new();
        let _gate = PushDlqDedupGate::from_env(&env);
        env.set(ENV_PUSH_DLQ_DEDUP_LRU_SIZE, "not-a-number");
        let _gate = PushDlqDedupGate::from_env(&env);
    }

    #[test]
    fn try_acquire_recovers_from_a_poisoned_lock() {
        use std::sync::Arc;
        let gate = Arc::new(PushDlqDedupGate::with_capacity(8));
        let poison = Arc::clone(&gate);
        let _ = std::thread::spawn(move || {
            let _guard = poison.state.lock().unwrap();
            panic!("poison the dedup lock");
        })
        .join();
        // Lock is poisoned now; try_acquire must still let new keys through.
        let k = dlq_key("t-poison", "failed", "https://example.com/hook");
        assert!(gate.try_acquire(&k));
    }

    #[test]
    fn default_uses_built_in_lru_size() {
        let gate = PushDlqDedupGate::default();
        let k = dlq_key("t-default", "failed", "https://example.com/hook");
        assert!(gate.try_acquire(&k));
    }

    #[test]
    fn lru_eviction_allows_reacquire_after_capacity() {
        let gate = PushDlqDedupGate::with_capacity(2);
        let k1 = dlq_key("t1", "failed", "https://a.example/hook");
        let k2 = dlq_key("t2", "failed", "https://b.example/hook");
        let k3 = dlq_key("t3", "failed", "https://c.example/hook");

        assert!(gate.try_acquire(&k1));
        assert!(gate.try_acquire(&k2));
        assert!(gate.try_acquire(&k3));
        assert!(gate.try_acquire(&k1));
    }
}
