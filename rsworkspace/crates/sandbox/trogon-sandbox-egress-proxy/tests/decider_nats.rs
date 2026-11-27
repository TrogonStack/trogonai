use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{DecideMethod, DecideRequest, EgressViolationReason, LeaseId};
use trogon_sandbox_contract::{Host, Port};
use trogon_sandbox_egress_proxy::decider::{Decider, DeciderLimits, Judgment, NatsDeciderTransport, PolicyAmender};
use trogon_sandbox_egress_proxy::policy::Decision;
use trogon_sandbox_protocol::subjects::egress_decide;

struct IgnoreAmendments;

#[async_trait]
impl PolicyAmender for IgnoreAmendments {
    async fn amend(&self, _host: &Host, _port: Port, _decision: Decision) {}
}

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn unique_lease() -> TestResult<LeaseId> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    Ok(LeaseId::new(format!("decide-{nanos}")))
}

async fn decider_over(client: async_nats::Client, lease: &LeaseId, timeout: Duration) -> Decider {
    Decider::new(
        lease.clone(),
        Arc::new(NatsDeciderTransport::new(client)),
        Arc::new(IgnoreAmendments),
        DeciderLimits {
            timeout,
            max_in_flight: 4,
        },
    )
}

#[tokio::test]
async fn a_responder_on_the_decide_subject_allows_and_denies() -> TestResult {
    let lease = unique_lease()?;
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let mut requests = client.subscribe(egress_decide(&lease)).await?;
    let responder = client.clone();
    tokio::spawn(async move {
        while let Some(message) = requests.next().await {
            let Ok(request) = serde_json::from_slice::<DecideRequest>(&message.payload) else {
                continue;
            };
            let verdict = if request.host.to_string() == "yes.test" {
                r#"{"verdict":"allow","scope":"once"}"#
            } else {
                r#"{"verdict":"deny","scope":"once"}"#
            };
            if let Some(reply) = message.reply {
                let _ = responder.publish(reply, verdict.into()).await;
            }
        }
    });
    client.flush().await?;

    let decider = decider_over(client, &lease, Duration::from_secs(5)).await;
    let port = Port::new(443)?;
    let yes = Host::new("yes.test")?;
    let no = Host::new("no.test")?;
    assert_eq!(
        decider.decide(&yes, port, Some(DecideMethod::Connect)).await,
        Judgment::Allow
    );
    assert_eq!(
        decider.decide(&no, port, Some(DecideMethod::Connect)).await,
        Judgment::Deny(EgressViolationReason::DeciderDenied)
    );
    Ok(())
}

#[tokio::test]
async fn no_responder_denies_instead_of_hanging() -> TestResult {
    let lease = unique_lease()?;
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let decider = decider_over(client, &lease, Duration::from_millis(500)).await;
    let judgment = decider.decide(&Host::new("quiet.test")?, Port::new(443)?, None).await;
    assert!(matches!(
        judgment,
        Judgment::Deny(EgressViolationReason::DeciderUnavailable | EgressViolationReason::DeciderTimedOut)
    ));
    Ok(())
}
