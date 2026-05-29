use std::time::Duration;

use async_nats::Client;
use futures::StreamExt;
use tracing::warn;

use crate::approvals::state::{ApprovalCache, ApprovalStateMachine};
use crate::approvals::types::{
    ApprovalDecisionMessage, ApprovalClientError, ApprovalSubject, ApprovalWaitOutcome, ArgsHash, RequestId,
};

pub struct ApprovalClient {
    client: Client,
    cache: ApprovalCache,
}

impl ApprovalClient {
    pub fn new(client: Client, cache: ApprovalCache) -> Self {
        Self { client, cache }
    }

    pub fn cache(&self) -> &ApprovalCache {
        &self.cache
    }

    pub async fn await_decision(
        &self,
        subject: &ApprovalSubject,
        request_id: &RequestId,
        args_hash: &ArgsHash,
        ttl: Duration,
    ) -> Result<ApprovalWaitOutcome, ApprovalClientError> {
        if self.cache.is_approved(request_id, args_hash).await {
            return Ok(ApprovalWaitOutcome::Approved {
                approver: "cached".into(),
                expires_at: ApprovalStateMachine::ttl_deadline(ttl),
            });
        }

        let mut subscription = self
            .client
            .subscribe(subject.as_str().to_string())
            .await
            .map_err(|e| ApprovalClientError::Subscribe(e.to_string()))?;

        let wait = tokio::time::timeout(ttl, async {
            while let Some(message) = subscription.next().await {
                let Ok(decision) = serde_json::from_slice::<ApprovalDecisionMessage>(&message.payload) else {
                    warn!(subject = subject.as_str(), "ignored malformed approval decision");
                    continue;
                };
                return ApprovalStateMachine::interpret(&decision, ttl.as_secs());
            }
            ApprovalWaitOutcome::TimedOut
        })
        .await;

        match wait {
            Ok(ApprovalWaitOutcome::Approved {
                approver,
                expires_at,
            }) => {
                self.cache
                    .store(request_id, args_hash, expires_at)
                    .await;
                Ok(ApprovalWaitOutcome::Approved {
                    approver,
                    expires_at,
                })
            }
            Ok(other) => Ok(other),
            Err(_) => Ok(ApprovalWaitOutcome::TimedOut),
        }
    }
}
