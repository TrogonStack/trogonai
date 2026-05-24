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
        let mut guard = self.state.lock().expect("push DLQ dedup lock");
        let (order, seen) = &mut *guard;

        if seen.contains(&key) {
            return false;
        }

        while seen.len() >= self.capacity {
            let Some(evicted) = order.pop_front() else {
                break;
            };
            seen.remove(&evicted);
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
