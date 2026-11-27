//! The redispatch checks pass a lease handler that keeps its grants in a `RedispatchGuard` and catch the two ways a
//! handler goes wrong: launching again for a repeat, and answering a reused id with the sandbox a cancel released.

use std::error::Error;
use std::future::Future;
use std::sync::Arc;

use async_nats::{Client, Message};
use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_conformance::{
    CountingProvider, RedispatchError, ensure_launches_once, ensure_relaunches_after_cancel,
};
use trogon_sandbox_contract::{LeaseId, ProviderDispatchPayload, ProviderKind};
use trogon_sandbox_protocol::{Provider, RedispatchGuard};

type BoxError = Box<dyn Error + Send + Sync>;

/// A lease handler over `provider`, guarded by `guard` when one is given.
fn handler(
    client: Client,
    provider: Arc<CountingProvider>,
    guard: Option<Arc<RedispatchGuard>>,
) -> impl Fn(Message) -> std::pin::Pin<Box<dyn Future<Output = Result<(), BoxError>> + Send>> {
    move |message: Message| {
        let client = client.clone();
        let provider = provider.clone();
        let guard = guard.clone();
        Box::pin(async move {
            let payload: ProviderDispatchPayload = serde_json::from_slice(&message.payload)?;
            let launch = || provider.lease(&payload.request, &payload.lease_id);
            let lease = match &guard {
                Some(guard) => guard.get_or_launch(&payload.lease_id, launch).await?,
                None => launch().await?,
            };
            let reply = message.reply.ok_or("the dispatch carries no reply subject")?;
            client.publish(reply, serde_json::to_vec(&lease)?.into()).await?;
            Ok(())
        })
    }
}

/// A cancel handler that forgets the grant when `forget` is set.
fn canceller(
    guard: Arc<RedispatchGuard>,
    lease_id: LeaseId,
    forget: bool,
) -> impl Fn(Message) -> std::future::Ready<Result<(), BoxError>> {
    move |_message: Message| {
        if forget {
            guard.forget(&lease_id);
        }
        std::future::ready(Ok(()))
    }
}

async fn connect(server: &CoreTestServer) -> Result<Client, Box<dyn Error>> {
    Ok(async_nats::connect(server.address()).await?)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_guarded_handler_launches_once_for_a_repeat() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = connect(&server).await?;
    let provider = CountingProvider::new(ProviderKind::Mock);
    let guard = Arc::new(RedispatchGuard::new());
    let handle = handler(client.clone(), provider.clone(), Some(guard));
    ensure_launches_once(&client, &provider, &LeaseId::new("redispatch-once"), handle).await?;
    assert_eq!(provider.launches(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unguarded_handler_is_caught_launching_twice() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = connect(&server).await?;
    let provider = CountingProvider::new(ProviderKind::Mock);
    let handle = handler(client.clone(), provider.clone(), None);
    let outcome = ensure_launches_once(&client, &provider, &LeaseId::new("redispatch-twice"), handle).await;
    assert!(
        matches!(outcome, Err(RedispatchError::RepeatGrantDiffers { .. })),
        "{outcome:?}"
    );
    assert_eq!(provider.launches(), 2);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_that_forgets_the_grant_relaunches_a_reused_id() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = connect(&server).await?;
    let provider = CountingProvider::new(ProviderKind::Mock);
    let guard = Arc::new(RedispatchGuard::new());
    let lease_id = LeaseId::new("redispatch-cancel");
    let handle = handler(client.clone(), provider.clone(), Some(guard.clone()));
    let cancel = canceller(guard, lease_id.clone(), true);
    ensure_relaunches_after_cancel(&client, &provider, &lease_id, handle, cancel).await?;
    assert_eq!(provider.launches(), 2);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_that_keeps_the_grant_is_caught_reusing_the_sandbox() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = connect(&server).await?;
    let provider = CountingProvider::new(ProviderKind::Mock);
    let guard = Arc::new(RedispatchGuard::new());
    let lease_id = LeaseId::new("redispatch-kept");
    let handle = handler(client.clone(), provider.clone(), Some(guard.clone()));
    let cancel = canceller(guard, lease_id.clone(), false);
    let outcome = ensure_relaunches_after_cancel(&client, &provider, &lease_id, handle, cancel).await;
    assert!(
        matches!(outcome, Err(RedispatchError::ReleasedSandboxReused(_))),
        "{outcome:?}"
    );
    assert_eq!(provider.launches(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_handler_that_never_replies_is_reported() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = connect(&server).await?;
    let provider = CountingProvider::new(ProviderKind::Mock);
    let silent = |_message: Message| std::future::ready(Ok::<(), BoxError>(()));
    let outcome = ensure_launches_once(&client, &provider, &LeaseId::new("redispatch-silent"), silent).await;
    assert!(matches!(outcome, Err(RedispatchError::NoReply(_))), "{outcome:?}");
    Ok(())
}
