mod support;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use support::{Server, TestResult, json, respond, start_nats};
use trogon_sandbox_contract::{LeaseId, TenantId};
use trogon_sandbox_protocol::{AUTH_HEADER, DispatchSigner};
use trogon_sandbox_ssh_gateway::{
    NatsSshTunnelBackend, SshAttachRefusal, SshAttachRequest, SshAttachResponse, SshDetachRequest, SshDetachResponse,
    SshSessionId, SshStreamFrame, SshTunnelBackend, TunnelError,
};

const ATTACH: &str = "sandbox.ssh.attach.lease-1";
const DETACH: &str = "sandbox.ssh.detach.lease-1";

fn attach_request() -> TestResult<SshAttachRequest> {
    Ok(SshAttachRequest {
        session_id: "s1".parse()?,
        lease_id: LeaseId::new("lease-1"),
        public_key: "ssh-ed25519 AAAA".to_string(),
        ttl_seconds: std::num::NonZeroU32::new(60).ok_or("zero")?,
    })
}

fn detach_request() -> TestResult<SshDetachRequest> {
    Ok(SshDetachRequest {
        session_id: "s1".parse()?,
        lease_id: LeaseId::new("lease-1"),
        reason: "done".to_string(),
    })
}

fn tenant() -> TestResult<TenantId> {
    Ok(TenantId::new("tenant-a")?)
}

fn frame(session: &str, data: &[u8]) -> TestResult<SshStreamFrame> {
    Ok(SshStreamFrame::data(
        &session.parse::<SshSessionId>()?,
        1,
        data.to_vec(),
    ))
}

#[tokio::test]
async fn attach_reaches_the_worker_and_is_accepted() -> TestResult {
    let (_server, client) = start_nats().await?;
    let mut seen = respond(&client, ATTACH, |_| json(&SshAttachResponse::accepted()).ok()).await?;
    let backend = NatsSshTunnelBackend::new(client);
    let response = backend.attach(&tenant()?, &attach_request()?).await?;
    assert!(response.ok);
    let request = seen.recv().await.ok_or("no request")?;
    let sent: SshAttachRequest = serde_json::from_slice(&request.payload)?;
    assert_eq!(sent, attach_request()?);
    assert!(request.headers.is_none());
    Ok(())
}

#[tokio::test]
async fn attach_and_detach_are_signed_when_a_signer_is_set() -> TestResult {
    let (_server, client) = start_nats().await?;
    let mut attaches = respond(&client, ATTACH, |_| json(&SshAttachResponse::accepted()).ok()).await?;
    let mut detaches = respond(&client, DETACH, |_| json(&SshDetachResponse::accepted()).ok()).await?;
    let signer = Arc::new(DispatchSigner::new("key-1", [7u8; 32]));
    let backend = NatsSshTunnelBackend::new(client).with_signer(signer);
    backend.attach(&tenant()?, &attach_request()?).await?;
    backend.detach(&tenant()?, &detach_request()?).await?;
    for seen in [&mut attaches, &mut detaches] {
        let message = seen.recv().await.ok_or("no request")?;
        let headers = message.headers.ok_or("unsigned request")?;
        assert!(headers.get(AUTH_HEADER).is_some());
    }
    Ok(())
}

#[tokio::test]
async fn a_refused_attach_names_the_worker_message() -> TestResult {
    let (_server, client) = start_nats().await?;
    respond(&client, ATTACH, |_| json(&SshAttachResponse::rejected("busy")).ok()).await?;
    let backend = NatsSshTunnelBackend::new(client);
    let error = backend.attach(&tenant()?, &attach_request()?).await.err();
    assert!(matches!(error, Some(TunnelError::AttachRejected(message)) if message == "busy"));
    Ok(())
}

#[tokio::test]
async fn a_refusal_without_a_message_gets_a_default() -> TestResult {
    let (_server, client) = start_nats().await?;
    let silent = SshAttachResponse {
        ok: false,
        message: None,
        refusal: None,
    };
    respond(&client, ATTACH, move |_| json(&silent).ok()).await?;
    let backend = NatsSshTunnelBackend::new(client);
    let error = backend.attach(&tenant()?, &attach_request()?).await.err();
    assert!(matches!(error, Some(TunnelError::AttachRejected(message)) if message == "attach denied"));
    Ok(())
}

#[tokio::test]
async fn an_unsupported_lease_is_typed() -> TestResult {
    let (_server, client) = start_nats().await?;
    respond(&client, ATTACH, |_| {
        json(&SshAttachResponse::unsupported("no ssh")).ok()
    })
    .await?;
    let backend = NatsSshTunnelBackend::new(client);
    let error = backend.attach(&tenant()?, &attach_request()?).await.err();
    assert!(matches!(error, Some(TunnelError::Unsupported(message)) if message == "no ssh"));
    assert_eq!(SshAttachRefusal::Unsupported, SshAttachRefusal::Unsupported);
    Ok(())
}

#[tokio::test]
async fn a_refused_detach_names_the_worker_message() -> TestResult {
    let (_server, client) = start_nats().await?;
    respond(&client, DETACH, |_| json(&SshDetachResponse::rejected("unknown")).ok()).await?;
    let backend = NatsSshTunnelBackend::new(client);
    let error = backend.detach(&tenant()?, &detach_request()?).await.err();
    assert!(matches!(error, Some(TunnelError::DetachRejected(message)) if message == "unknown"));
    Ok(())
}

#[tokio::test]
async fn a_detach_refusal_without_a_message_gets_a_default() -> TestResult {
    let (_server, client) = start_nats().await?;
    let silent = SshDetachResponse {
        ok: false,
        message: None,
    };
    respond(&client, DETACH, move |_| json(&silent).ok()).await?;
    let backend = NatsSshTunnelBackend::new(client);
    let error = backend.detach(&tenant()?, &detach_request()?).await.err();
    assert!(matches!(error, Some(TunnelError::DetachRejected(message)) if message == "detach denied"));
    Ok(())
}

#[tokio::test]
async fn a_silent_worker_times_out() -> TestResult {
    let (_server, client) = start_nats().await?;
    respond(&client, ATTACH, |_| None).await?;
    let backend = NatsSshTunnelBackend::new(client).with_timeout(Duration::from_millis(100));
    let error = backend.attach(&tenant()?, &attach_request()?).await.err();
    assert!(matches!(error, Some(TunnelError::Timeout(wait)) if wait == Duration::from_millis(100)));
    Ok(())
}

#[tokio::test]
async fn a_missing_worker_fails_the_request() -> TestResult {
    let (_server, client) = start_nats().await?;
    let backend = NatsSshTunnelBackend::new(client);
    let error = backend.attach(&tenant()?, &attach_request()?).await.err();
    assert!(matches!(error, Some(TunnelError::Request(_))));
    Ok(())
}

#[tokio::test]
async fn a_garbled_reply_is_a_decode_error() -> TestResult {
    let (_server, client) = start_nats().await?;
    respond(&client, ATTACH, |_| Some(Bytes::from_static(b"not json"))).await?;
    let backend = NatsSshTunnelBackend::new(client);
    let error = backend.attach(&tenant()?, &attach_request()?).await.err();
    assert!(matches!(error, Some(TunnelError::Decode(_))));
    Ok(())
}

#[tokio::test]
async fn frames_flow_both_ways() -> TestResult {
    let (_server, client) = start_nats().await?;
    let backend = NatsSshTunnelBackend::new(client.clone());
    let session: SshSessionId = "s1".parse()?;

    let mut to_worker = respond(&client, "sandbox.ssh.stream.s1.to_worker", |_| None).await?;
    backend.publish_frame(&frame("s1", b"input")?).await?;
    let delivered = to_worker.recv().await.ok_or("nothing delivered")?;
    assert_eq!(
        serde_json::from_slice::<SshStreamFrame>(&delivered.payload)?,
        frame("s1", b"input")?
    );

    let mut from_worker = backend.subscribe_frames(&session).await?;
    client.flush().await?;
    client
        .publish("sandbox.ssh.stream.s1.from_worker", Bytes::from_static(b"garbage"))
        .await?;
    client
        .publish("sandbox.ssh.stream.s1.from_worker", json(&frame("s1", b"output")?)?)
        .await?;
    assert_eq!(from_worker.recv().await.ok_or("no frame")?, frame("s1", b"output")?);
    Ok(())
}

#[tokio::test]
async fn a_dropped_receiver_ends_the_subscription_task() -> TestResult {
    let (_server, client) = start_nats().await?;
    let backend = NatsSshTunnelBackend::new(client.clone());
    let session: SshSessionId = "s2".parse()?;
    let receiver = backend.subscribe_frames(&session).await?;
    client.flush().await?;
    drop(receiver);
    tokio::time::sleep(Duration::from_millis(100)).await;
    Ok(())
}

#[tokio::test]
async fn a_full_buffer_and_a_dropped_receiver_end_the_task() -> TestResult {
    let (_server, client) = start_nats().await?;
    let backend = NatsSshTunnelBackend::new(client.clone());
    let session: SshSessionId = "s3".parse()?;
    let receiver = backend.subscribe_frames(&session).await?;
    client.flush().await?;
    let body = json(&frame("s3", b"x")?)?;
    for _ in 0..400 {
        client
            .publish("sandbox.ssh.stream.s3.from_worker", body.clone())
            .await?;
    }
    client.flush().await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(receiver);
    tokio::time::sleep(Duration::from_millis(100)).await;
    Ok(())
}

#[tokio::test]
async fn a_closed_client_fails_publish_and_subscribe() -> TestResult {
    let (_server, client) = start_nats().await?;
    let backend = NatsSshTunnelBackend::new(client.clone());
    client.drain().await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let session: SshSessionId = "s4".parse()?;
    assert!(matches!(
        backend.subscribe_frames(&session).await.err(),
        Some(TunnelError::Subscribe(_))
    ));
    assert!(matches!(
        backend.publish_frame(&frame("s4", b"x")?).await.err(),
        Some(TunnelError::Publish(_))
    ));
    Ok(())
}

#[tokio::test]
async fn an_oversized_frame_fails_to_publish() -> TestResult {
    let server = Server::start_with_config("max_payload: 1024\n").await;
    let client = server.client().await?;
    let backend = NatsSshTunnelBackend::new(client);
    let error = backend.publish_frame(&frame("s5", &[b'x'; 4096])?).await.err();
    assert!(matches!(error, Some(TunnelError::Publish(_))));
    Ok(())
}
