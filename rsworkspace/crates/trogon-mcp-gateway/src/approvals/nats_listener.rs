use std::sync::Arc;

use async_nats::Client;
use futures::StreamExt;
use tokio::task::JoinHandle;
use tracing::warn;

use crate::approvals::coordinator::ApprovalCoordinator;
use crate::approvals::types::{ApprovalDecisionMessage, wire_to_decision};

pub struct ApprovalNatsListener;

impl ApprovalNatsListener {
    pub fn start(
        client: Client,
        subject_prefix: &str,
        coordinator: Arc<ApprovalCoordinator>,
    ) -> JoinHandle<()> {
        let subject_prefix = subject_prefix.to_string();
        let wildcard = format!("{subject_prefix}.>");
        tokio::spawn(async move {
            let mut subscription = match client.subscribe(wildcard.clone()).await {
                Ok(subscription) => subscription,
                Err(error) => {
                    warn!(subject = %wildcard, %error, "approval listener subscribe failed");
                    return;
                }
            };

            while let Some(message) = subscription.next().await {
                let Some(request_id) =
                    request_id_from_subject(message.subject.as_str(), subject_prefix.as_str())
                else {
                    warn!(subject = %message.subject, "ignored approval subject outside prefix");
                    continue;
                };

                let Ok(decision_message) =
                    serde_json::from_slice::<ApprovalDecisionMessage>(&message.payload)
                else {
                    warn!(subject = %message.subject, "ignored malformed approval decision");
                    continue;
                };

                let Ok(decision) = wire_to_decision(&decision_message) else {
                    warn!(subject = %message.subject, "ignored invalid approval decision");
                    continue;
                };

                if !coordinator.fulfill(&request_id, decision) {
                    warn!(request_id, "approval decision with no pending waiter");
                }
            }
        })
    }
}

fn request_id_from_subject(subject: &str, subject_prefix: &str) -> Option<String> {
    let suffix = subject.strip_prefix(subject_prefix)?.strip_prefix('.')?;
    if suffix.is_empty() {
        return None;
    }
    Some(suffix.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_request_id_from_subject() {
        assert_eq!(
            request_id_from_subject("mcp.approvals.req-abc", "mcp.approvals").as_deref(),
            Some("req-abc")
        );
    }

    #[test]
    fn extracts_step_up_request_id_from_subject() {
        assert_eq!(
            request_id_from_subject("mcp.approvals.step-up.req-abc", "mcp.approvals").as_deref(),
            Some("step-up.req-abc")
        );
    }
}
