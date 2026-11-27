//! Checks that a worker launches once when the broker re-sends a lease id after a lost reply.
//!
//! Each worker wires `RedispatchGuard` into its own lease handler, so the checks take that handler as a closure and
//! drive it twice with the same lease id, with a [`CountingProvider`] behind it to count the launches.

use std::error::Error;
use std::future::Future;

use async_nats::{Client, Message};
use futures::StreamExt;
use trogon_sandbox_contract::{LeaseId, LeaseRequest, ProviderDispatchPayload, ProviderKind, ProviderLease};
use trogon_sandbox_protocol::Provider;
use trogon_sandbox_protocol::subjects::{provider_cancel, provider_dispatch};

use crate::constants::REDISPATCH_REPLY_TIMEOUT;
use crate::{CountingProvider, RedispatchError, Target};

type BoxError = Box<dyn Error + Send + Sync>;

/// The request every dispatch of these checks carries.
fn redispatch_request(client: &Client, kind: ProviderKind) -> Result<LeaseRequest, RedispatchError> {
    Ok(Target::new(client.clone(), kind).lease_request()?)
}

fn message(subject: String, reply: Option<String>, payload: Vec<u8>) -> Message {
    Message {
        subject: subject.into(),
        reply: reply.map(Into::into),
        length: payload.len(),
        payload: payload.into(),
        headers: None,
        status: None,
        description: None,
    }
}

/// Sends one dispatch of `request` for `lease_id` through `handle` and returns the grant it replied with.
pub async fn dispatch_lease<F, Fut, E>(
    client: &Client,
    kind: ProviderKind,
    request: &LeaseRequest,
    lease_id: &LeaseId,
    handle: &F,
) -> Result<ProviderLease, RedispatchError>
where
    F: Fn(Message) -> Fut,
    Fut: Future<Output = Result<(), E>>,
    E: Into<BoxError>,
{
    let payload = ProviderDispatchPayload {
        lease_id: lease_id.clone(),
        request: request.clone(),
        correlation_id: None,
        resolved_drives: vec![],
        target: None,
    };
    let body = serde_json::to_vec(&payload).map_err(RedispatchError::Encode)?;
    let inbox = client.new_inbox();
    let mut replies = client
        .subscribe(inbox.clone())
        .await
        .map_err(RedispatchError::Subscribe)?;
    handle(message(provider_dispatch(kind), Some(inbox), body))
        .await
        .map_err(|error| RedispatchError::Handler(error.into()))?;
    let reply = tokio::time::timeout(REDISPATCH_REPLY_TIMEOUT, replies.next())
        .await
        .ok()
        .flatten()
        .ok_or(RedispatchError::NoReply(REDISPATCH_REPLY_TIMEOUT))?;
    serde_json::from_slice(&reply.payload).map_err(RedispatchError::Decode)
}

fn ensure_launches(provider: &CountingProvider, expected: u32) -> Result<(), RedispatchError> {
    let observed = provider.launches();
    if observed == expected {
        return Ok(());
    }
    Err(RedispatchError::LaunchCount { expected, observed })
}

/// Dispatches the same lease id twice and checks the worker launched once and handed the repeat the first grant.
pub async fn ensure_launches_once<F, Fut, E>(
    client: &Client,
    provider: &CountingProvider,
    lease_id: &LeaseId,
    handle: F,
) -> Result<(), RedispatchError>
where
    F: Fn(Message) -> Fut,
    Fut: Future<Output = Result<(), E>>,
    E: Into<BoxError>,
{
    let kind = provider.kind();
    let request = redispatch_request(client, kind)?;
    let first = dispatch_lease(client, kind, &request, lease_id, &handle).await?;
    let second = dispatch_lease(client, kind, &request, lease_id, &handle).await?;
    if first != second {
        return Err(RedispatchError::RepeatGrantDiffers {
            first: Box::new(first),
            second: Box::new(second),
        });
    }
    ensure_launches(provider, 1)
}

/// Dispatches a lease id, cancels it through `cancel`, then dispatches the same id again and checks the worker
/// launched a second time: a cancel forgets the grant, so a reused id must not be answered with the released sandbox.
pub async fn ensure_relaunches_after_cancel<F, Fut, E, C, CFut, CE>(
    client: &Client,
    provider: &CountingProvider,
    lease_id: &LeaseId,
    handle: F,
    cancel: C,
) -> Result<(), RedispatchError>
where
    F: Fn(Message) -> Fut,
    Fut: Future<Output = Result<(), E>>,
    E: Into<BoxError>,
    C: Fn(Message) -> CFut,
    CFut: Future<Output = Result<(), CE>>,
    CE: Into<BoxError>,
{
    let kind = provider.kind();
    let request = redispatch_request(client, kind)?;
    let first = dispatch_lease(client, kind, &request, lease_id, &handle).await?;
    cancel(message(provider_cancel(kind, lease_id), None, Vec::new()))
        .await
        .map_err(|error| RedispatchError::Handler(error.into()))?;
    let second = dispatch_lease(client, kind, &request, lease_id, &handle).await?;
    if first.sandbox_id == second.sandbox_id {
        return Err(RedispatchError::ReleasedSandboxReused(second.sandbox_id));
    }
    ensure_launches(provider, 2)
}
