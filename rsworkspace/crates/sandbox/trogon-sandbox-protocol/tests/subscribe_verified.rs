//! Worker subscriptions that verify signed requests, against a real `nats-server`.

mod nats_common;

use std::error::Error;
use std::time::Duration;

use async_nats::{Client, HeaderMap};
use futures::{Stream, StreamExt as _};
use serde_json::{Value, json};
use trogon_sandbox_contract::{LeaseId, TenantId};
use trogon_sandbox_protocol::{
    AUTH_HEADER, DispatchSigner, DispatchVerifier, EnforcementMode, OP_LEASE_DISPATCH, VerbScope,
    queue_subscribe_verified, subscribe_verified, subscribe_verified_context, subscribe_with, subscribe_with_context,
};

const WAIT: Duration = Duration::from_secs(10);

fn pair(mode: EnforcementMode) -> Result<(DispatchSigner, &'static DispatchVerifier), Box<dyn Error>> {
    let signer = DispatchSigner::new("k1", [41; 32]);
    let verifier = DispatchVerifier::new(mode).with_key(signer.key_id(), &signer.public_key_b64())?;
    Ok((signer, Box::leak(Box::new(verifier))))
}

fn new_lease() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// The worker has seen the lease dispatched for `tenant`.
fn admit_dispatch(
    signer: &DispatchSigner,
    verifier: &DispatchVerifier,
    lease: &str,
    tenant: &str,
) -> Result<(), Box<dyn Error>> {
    let payload = serde_json::to_vec(&json!({"lease_id": lease, "request": {"tenant": tenant}}))?;
    let signed = signer.sign_payload(&payload, OP_LEASE_DISPATCH)?;
    verifier.decode::<Value>(&signed, OP_LEASE_DISPATCH, None)?;
    Ok(())
}

fn scope(tenant: &str, lease: &str) -> Result<VerbScope, Box<dyn Error>> {
    Ok(VerbScope {
        tenant: TenantId::new(tenant)?,
        lease_id: LeaseId::new(lease),
        worker_id: None,
        attempt_id: None,
        epoch: None,
    })
}

async fn send(client: &Client, subject: &str, auth: Option<&str>, body: &[u8]) -> Result<(), Box<dyn Error>> {
    let mut headers = HeaderMap::new();
    if let Some(auth) = auth {
        headers.insert(AUTH_HEADER, auth);
    }
    client
        .publish_with_headers(subject.to_owned(), headers, body.to_vec().into())
        .await?;
    Ok(())
}

async fn next<S: Stream + Unpin>(stream: &mut S) -> Result<S::Item, Box<dyn Error>> {
    tokio::time::timeout(WAIT, stream.next())
        .await?
        .ok_or_else(|| "the subscription closed".into())
}

#[tokio::test]
async fn require_drops_unsigned_forged_replayed_and_foreign_tenant_requests() -> Result<(), Box<dyn Error>> {
    let (_server, client) = nats_common::start_nats().await?;
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let lease = new_lease();
    let subject = format!("sandbox.provider.firecracker.run_command.{lease}");
    let mut sub = subscribe_with(&client, subject.clone(), verifier).await?;
    let plain = format!("sandbox.provider.firecracker.heartbeat.{lease}");
    let mut plain_sub = subscribe_with(&client, plain.clone(), verifier).await?;
    client.flush().await?;
    admit_dispatch(&signer, verifier, &lease, "t1")?;

    let dropped: &[u8] = br#"{"command":["dropped"]}"#;
    let delivered: &[u8] = br#"{"command":["ls"]}"#;
    let sentinel: &[u8] = br#"{"command":["sentinel"]}"#;
    let own = scope("t1", &lease)?;
    let signed = signer.sign_verb(&subject, delivered, &own)?;
    let forged = DispatchSigner::new("k1", [42; 32]).sign_verb(&subject, dropped, &own)?;
    let foreign_tenant = signer.sign_verb(&subject, dropped, &scope("t2", &lease)?)?;
    let signed_for_other_body = signer.sign_verb(&subject, delivered, &own)?;
    let last = signer.sign_verb(&subject, sentinel, &own)?;

    send(&client, &subject, None, dropped).await?;
    send(&client, &subject, Some(&forged), dropped).await?;
    send(&client, &subject, Some(&signed), delivered).await?;
    // The same signed request again, a signature for another tenant, and a body that differs from the signed one.
    send(&client, &subject, Some(&signed), delivered).await?;
    send(&client, &subject, Some(&foreign_tenant), dropped).await?;
    send(&client, &subject, Some(&signed_for_other_body), dropped).await?;
    send(&client, &subject, Some(&last), sentinel).await?;
    // A request that is not a signed subject is never checked.
    send(&client, &plain, None, dropped).await?;

    let first = next(&mut sub).await?;
    assert_eq!(first.payload.as_ref(), delivered);
    assert!(first.headers.ok_or("headers kept")?.get(AUTH_HEADER).is_some());
    // Everything between the first and the sentinel was refused.
    let second = next(&mut sub).await?;
    assert_eq!(second.payload.as_ref(), sentinel);
    let passthrough = next(&mut plain_sub).await?;
    assert_eq!(passthrough.payload.as_ref(), dropped);
    Ok(())
}

#[tokio::test]
async fn a_refused_request_gets_an_error_reply() -> Result<(), Box<dyn Error>> {
    let (_server, client) = nats_common::start_nats().await?;
    let (_signer, verifier) = pair(EnforcementMode::Require)?;
    let subject = "sandbox.provider.firecracker.pause.l9";
    let mut sub = subscribe_with(&client, subject, verifier).await?;
    client.flush().await?;
    let handler = tokio::spawn(async move { while sub.next().await.is_some() {} });
    let reply = tokio::time::timeout(WAIT, client.request(subject, "{}".into())).await??;
    handler.abort();
    let body: Value = serde_json::from_slice(&reply.payload)?;
    assert_eq!(body["error"], "dispatch_auth");
    assert!(body["message"].as_str().is_some_and(|message| !message.is_empty()));
    Ok(())
}

#[tokio::test]
async fn a_refused_request_without_a_reply_subject_is_dropped_silently() -> Result<(), Box<dyn Error>> {
    let (_server, client) = nats_common::start_nats().await?;
    let (_signer, verifier) = pair(EnforcementMode::Require)?;
    let subject = "sandbox.provider.firecracker.resume.l5";
    let mut sub = subscribe_with(&client, subject, verifier).await?;
    client.flush().await?;
    client.publish(subject, "{}".into()).await?;
    // Admitted request on a subject the verifier does not classify, published after the refused one.
    let mut other = subscribe_with(&client, "sandbox.provider.firecracker.other", verifier).await?;
    client.flush().await?;
    client.publish("sandbox.provider.firecracker.other", "x".into()).await?;
    let message = next(&mut other).await?;
    assert_eq!(message.payload.as_ref(), b"x");
    assert!(
        tokio::time::timeout(Duration::from_millis(200), sub.next())
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn warn_mode_admits_requests_that_fail_verification() -> Result<(), Box<dyn Error>> {
    let (_server, client) = nats_common::start_nats().await?;
    let (_signer, verifier) = pair(EnforcementMode::Warn)?;
    let subject = "sandbox.provider.firecracker.pause.l7";
    let mut sub = subscribe_with(&client, subject, verifier).await?;
    client.flush().await?;
    send(&client, subject, None, b"{}").await?;
    assert_eq!(next(&mut sub).await?.payload.as_ref(), b"{}");
    Ok(())
}

#[tokio::test]
async fn the_context_subscriber_yields_each_verified_scope() -> Result<(), Box<dyn Error>> {
    let (_server, client) = nats_common::start_nats().await?;
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let lease = new_lease();
    let subject = format!("sandbox.provider.firecracker.run_command.{lease}");
    let mut sub = subscribe_with_context(&client, subject.clone(), verifier).await?;
    let plain = format!("sandbox.provider.firecracker.heartbeat.{lease}");
    let mut plain_sub = subscribe_with_context(&client, plain.clone(), verifier).await?;
    client.flush().await?;

    let body: &[u8] = br#"{"command":["ls"]}"#;
    let expected = scope("t1", &lease)?;
    let signed = signer.sign_verb(&subject, body, &expected)?;
    send(&client, &subject, None, b"{}").await?;
    send(&client, &subject, Some(&signed), body).await?;
    send(&client, &plain, None, body).await?;

    let request = next(&mut sub).await?;
    assert_eq!(request.message.payload.as_ref(), body);
    let verified = request.scope.ok_or("a verified scope")?;
    assert_eq!(verified.scope(), &expected);
    assert!(verified.matches(&subject, body));
    assert!(!verified.matches(&subject, b"{}"));

    let unclassified = next(&mut plain_sub).await?;
    assert_eq!(unclassified.message.payload.as_ref(), body);
    assert!(unclassified.scope.is_none());
    Ok(())
}

#[tokio::test]
async fn the_context_subscriber_replies_to_refused_requests() -> Result<(), Box<dyn Error>> {
    let (_server, client) = nats_common::start_nats().await?;
    let (_signer, verifier) = pair(EnforcementMode::Require)?;
    let subject = "sandbox.provider.firecracker.pause.l3";
    let mut sub = subscribe_with_context(&client, subject, verifier).await?;
    client.flush().await?;
    let handler = tokio::spawn(async move { while sub.next().await.is_some() {} });
    let reply = tokio::time::timeout(WAIT, client.request(subject, "{}".into())).await??;
    handler.abort();
    let body: Value = serde_json::from_slice(&reply.payload)?;
    assert_eq!(body["error"], "dispatch_auth");
    Ok(())
}

#[tokio::test]
async fn unsubscribing_ends_the_stream() -> Result<(), Box<dyn Error>> {
    let (_server, client) = nats_common::start_nats().await?;
    let (_signer, verifier) = pair(EnforcementMode::Require)?;
    let mut sub = subscribe_with(&client, "sandbox.provider.firecracker.heartbeat.l1", verifier).await?;
    client.flush().await?;
    sub.unsubscribe().await?;
    client.flush().await?;
    let ended = tokio::time::timeout(WAIT, sub.next()).await?;
    assert!(ended.is_none());
    Ok(())
}

/// Without any verification environment the process-wide verifier is in warn mode with no keys, so every
/// message passes unverified.
#[tokio::test]
async fn process_wide_subscriptions_pass_messages_without_configured_keys() -> Result<(), Box<dyn Error>> {
    let (_server, client) = nats_common::start_nats().await?;
    let subject = "sandbox.provider.firecracker.pause.global1";
    let mut plain = subscribe_verified(&client, subject).await?;
    let mut context = subscribe_verified_context(&client, subject).await?;
    let mut queued = queue_subscribe_verified(&client, subject, "workers").await?;
    client.flush().await?;
    send(&client, subject, None, b"{}").await?;

    assert_eq!(next(&mut plain).await?.payload.as_ref(), b"{}");
    let request = next(&mut context).await?;
    assert_eq!(request.message.payload.as_ref(), b"{}");
    assert!(request.scope.is_none());
    assert_eq!(next(&mut queued).await?.payload.as_ref(), b"{}");
    Ok(())
}
