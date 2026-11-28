//! The worker's lease and cancel handlers, driven through the conformance redispatch checks.

use std::sync::Arc;

use async_nats::Message;
use axum::Router;
use futures::FutureExt;
use futures::future::BoxFuture;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_conformance::{CountingProvider, ensure_launches_once, ensure_relaunches_after_cancel};
use trogon_sandbox_contract::{LeaseId, ProviderKind};
use trogon_sandbox_protocol::RedispatchGuard;

use super::super::handler_error::HandlerError;
use super::super::{handle_cancel_msg, handle_lease_msg};
use super::support::{TestResult, create_evidence_stream, provider_holding};

/// The worker's own lease handler over `provider`, as the redispatch checks drive it.
fn lease_handler(
    client: &async_nats::Client,
    js: &async_nats::jetstream::Context,
    provider: Arc<CountingProvider>,
    grants: Arc<RedispatchGuard>,
) -> impl Fn(Message) -> BoxFuture<'static, Result<(), HandlerError>> {
    let (client, js) = (client.clone(), js.clone());
    move |msg| {
        handle_lease_msg(
            msg,
            client.clone(),
            js.clone(),
            Arc::clone(&provider),
            Arc::clone(&grants),
        )
        .boxed()
    }
}

#[tokio::test]
async fn a_redispatched_lease_launches_once() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = server.jetstream().await;
    create_evidence_stream(&js).await?;
    let counting = CountingProvider::new(ProviderKind::Substrate);
    let handle = lease_handler(&client, &js, counting.clone(), Arc::new(RedispatchGuard::new()));

    ensure_launches_once(&client, &counting, &LeaseId::new("redispatch-lease"), handle).await?;
    Ok(())
}

/// The cancel goes to a substrate provider that holds no actor for the lease, so it ends nothing but still has to
/// forget the grant the lease handler recorded.
#[tokio::test]
async fn a_cancelled_lease_id_launches_afresh_when_dispatched_again() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = server.jetstream().await;
    create_evidence_stream(&js).await?;
    let counting = CountingProvider::new(ProviderKind::Substrate);
    let grants = Arc::new(RedispatchGuard::new());
    let handle = lease_handler(&client, &js, counting.clone(), Arc::clone(&grants));
    let (substrate, _control, _index) = provider_holding(Router::new(), &[]).await?;
    let cancel = move |msg: Message| {
        let (substrate, grants) = (Arc::clone(&substrate), Arc::clone(&grants));
        async move {
            handle_cancel_msg(&msg, &substrate, &grants).await;
            Ok::<(), std::convert::Infallible>(())
        }
    };

    ensure_relaunches_after_cancel(&client, &counting, &LeaseId::new("redispatch-cancel"), handle, cancel).await?;
    Ok(())
}
