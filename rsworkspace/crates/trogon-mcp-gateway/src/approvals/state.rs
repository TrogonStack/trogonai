use std::time::{Duration, SystemTime, UNIX_EPOCH};

use moka::future::Cache;

use crate::approvals::types::{ApprovalDecisionKind, ApprovalDecisionMessage, ApprovalWaitOutcome, ArgsHash, RequestId};

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct ApprovalCacheKey {
    request_id: String,
    args_hash: String,
}

#[derive(Clone)]
pub struct ApprovalCache {
    entries: Cache<ApprovalCacheKey, i64>,
}

impl ApprovalCache {
    pub fn new(max_entries: u64) -> Self {
        Self {
            entries: Cache::builder().max_capacity(max_entries).build(),
        }
    }

    pub async fn store(&self, request_id: &RequestId, args_hash: &ArgsHash, expires_at: i64) {
        let key = ApprovalCacheKey {
            request_id: request_id.as_str().to_string(),
            args_hash: args_hash.hex(),
        };
        self.entries.insert(key, expires_at).await;
    }

    pub async fn is_approved(&self, request_id: &RequestId, args_hash: &ArgsHash) -> bool {
        let key = ApprovalCacheKey {
            request_id: request_id.as_str().to_string(),
            args_hash: args_hash.hex(),
        };
        let Some(expires_at) = self.entries.get(&key).await else {
            return false;
        };
        now_unix() < expires_at
    }
}

pub struct ApprovalStateMachine;

impl ApprovalStateMachine {
    pub fn interpret(decision: &ApprovalDecisionMessage, ttl_s: u64) -> ApprovalWaitOutcome {
        let now = now_unix();
        if decision.expires_at <= now {
            return ApprovalWaitOutcome::TimedOut;
        }
        match decision.decision {
            ApprovalDecisionKind::Approve => ApprovalWaitOutcome::Approved {
                approver: decision.approver.clone(),
                expires_at: decision.expires_at.min(now + ttl_s as i64),
            },
            ApprovalDecisionKind::Deny => ApprovalWaitOutcome::Denied {
                reason: format!("denied by {}", decision.approver),
            },
        }
    }

    pub fn ttl_deadline(ttl: Duration) -> i64 {
        now_unix() + ttl.as_secs() as i64
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approvals::types::{ApprovalDecisionKind, ApprovalDecisionMessage, ArgsHash, RequestId};

    #[test]
    fn approve_transition() {
        let msg = ApprovalDecisionMessage {
            decision: ApprovalDecisionKind::Approve,
            approver: "alice".into(),
            expires_at: now_unix() + 300,
        };
        match ApprovalStateMachine::interpret(&msg, 300) {
            ApprovalWaitOutcome::Approved { approver, .. } => assert_eq!(approver, "alice"),
            other => panic!("expected approve, got {other:?}"),
        }
    }

    #[test]
    fn deny_transition() {
        let msg = ApprovalDecisionMessage {
            decision: ApprovalDecisionKind::Deny,
            approver: "bob".into(),
            expires_at: now_unix() + 300,
        };
        assert!(matches!(
            ApprovalStateMachine::interpret(&msg, 300),
            ApprovalWaitOutcome::Denied { .. }
        ));
    }

    #[test]
    fn expired_decision_times_out() {
        let msg = ApprovalDecisionMessage {
            decision: ApprovalDecisionKind::Approve,
            approver: "alice".into(),
            expires_at: now_unix() - 1,
        };
        assert!(matches!(
            ApprovalStateMachine::interpret(&msg, 300),
            ApprovalWaitOutcome::TimedOut
        ));
    }

    #[tokio::test]
    async fn cache_hit_within_ttl() {
        let cache = ApprovalCache::new(100);
        let request_id = RequestId::new("req-cache").unwrap();
        let args_hash = ArgsHash::from_json(&serde_json::json!({"tool": "deploy"}));
        cache
            .store(&request_id, &args_hash, now_unix() + 60)
            .await;
        assert!(cache.is_approved(&request_id, &args_hash).await);
    }

    #[tokio::test]
    async fn cache_miss_for_different_args() {
        let cache = ApprovalCache::new(100);
        let request_id = RequestId::new("req-cache2").unwrap();
        let args_hash = ArgsHash::from_json(&serde_json::json!({"tool": "deploy"}));
        cache
            .store(&request_id, &args_hash, now_unix() + 60)
            .await;
        let other = ArgsHash::from_json(&serde_json::json!({"tool": "delete"}));
        assert!(!cache.is_approved(&request_id, &other).await);
    }
}
