use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use tokio::sync::oneshot;

use crate::approvals::errors::ApprovalError;
use crate::approvals::types::{ApprovalDecision, RequestId};

type PendingSender = oneshot::Sender<ApprovalDecision>;

#[derive(Default)]
pub struct ApprovalCoordinator {
    pending: Mutex<HashMap<String, PendingSender>>,
}

impl ApprovalCoordinator {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn wait_for_decision(
        &self,
        request_id: &RequestId,
        ttl: Duration,
    ) -> Result<ApprovalDecision, ApprovalError> {
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self
                .pending
                .lock()
                .expect("approval coordinator mutex poisoned");
            pending.insert(request_id.as_str().to_string(), tx);
        }

        match tokio::time::timeout(ttl, rx).await {
            Ok(Ok(decision)) => Ok(decision),
            Ok(Err(_)) => {
                self.remove(request_id);
                Err(ApprovalError::ChannelClosed)
            }
            Err(_) => {
                self.remove(request_id);
                Err(ApprovalError::Timeout)
            }
        }
    }

    pub fn fulfill(&self, request_id: &str, decision: ApprovalDecision) -> bool {
        let sender = self
            .pending
            .lock()
            .expect("approval coordinator mutex poisoned")
            .remove(request_id);
        match sender {
            Some(tx) => tx.send(decision).is_ok(),
            None => false,
        }
    }

    fn remove(&self, request_id: &RequestId) {
        self.pending
            .lock()
            .expect("approval coordinator mutex poisoned")
            .remove(request_id.as_str());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::approvals::types::ApprovalDecision;

    #[tokio::test]
    async fn granted_decision_returns_to_waiter() {
        let coordinator = Arc::new(ApprovalCoordinator::new());
        let request_id = RequestId::new("req-granted").expect("request id");
        let coordinator_ref = Arc::clone(&coordinator);
        let waiter_id = request_id.clone();

        let waiter = tokio::spawn(async move {
            coordinator_ref
                .wait_for_decision(&waiter_id, Duration::from_secs(2))
                .await
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        let delivered = coordinator.fulfill(
            request_id.as_str(),
            ApprovalDecision::Granted {
                approver: "human:alice".into(),
                expires_at: 4_000_000_000,
            },
        );
        assert!(delivered);

        let decision = waiter.await.expect("join").expect("decision");
        assert!(matches!(
            decision,
            ApprovalDecision::Granted { approver, .. } if approver == "human:alice"
        ));
    }

    #[tokio::test]
    async fn denied_decision_returns_to_waiter() {
        let coordinator = Arc::new(ApprovalCoordinator::new());
        let request_id = RequestId::new("req-denied").expect("request id");
        let coordinator_ref = Arc::clone(&coordinator);
        let waiter_id = request_id.clone();

        let waiter = tokio::spawn(async move {
            coordinator_ref
                .wait_for_decision(&waiter_id, Duration::from_secs(2))
                .await
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(coordinator.fulfill(
            request_id.as_str(),
            ApprovalDecision::Denied {
                approver: "human:bob".into(),
                reason: "denied by human:bob".into(),
            },
        ));

        let decision = waiter.await.expect("join").expect("decision");
        assert!(matches!(decision, ApprovalDecision::Denied { .. }));
    }

    #[tokio::test]
    async fn ttl_expiry_returns_timeout() {
        let coordinator = ApprovalCoordinator::new();
        let request_id = RequestId::new("req-timeout").expect("request id");

        let outcome = coordinator
            .wait_for_decision(&request_id, Duration::from_millis(50))
            .await;

        assert_eq!(outcome, Err(ApprovalError::Timeout));
    }
}
