//! SSH access, terminals and the SSH stream bridge on a running worker, against an in-process SSH server standing in
//! for Daytona's SSH gateway.

mod gateway_support;
#[allow(dead_code, reason = "shared test support; this binary uses a subset")]
mod nats_support;
mod support;

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use futures::StreamExt;
use gateway_support::{GatewayServer, TestResult, request_until_answered, unanswered};
use nats_support::Running;
use serde_json::json;
use trogon_sandbox_contract::{
    LeaseId, ProviderKind, TerminalDimension, TerminalId, TerminalOp, TerminalOutput, TerminalRequest, TerminalResponse,
};
use trogon_sandbox_protocol::subjects::{provider_terminal, terminal_output};
use trogon_sandbox_protocol::terminal_output_bytes;
use trogon_sandbox_ssh_gateway::{
    EphemeralSessionKey, SshAttachRequest, SshAttachResponse, SshDetachRequest, SshDetachResponse, SshSessionId,
    ssh_attach, ssh_detach,
};

const LEASE: &str = "lease-1";

/// A worker holding `lease-1` whose Daytona hands out access to `server`.
async fn running_with(server: &GatewayServer) -> TestResult<Running> {
    let running = Running::start().await?;
    running.dispatch(LEASE).await?;
    server.accept_token("tok-1");
    running.fake.json(
        "POST",
        &format!("/sandbox/{}/ssh-access", support::SANDBOX),
        200,
        json!({
            "token": "tok-1",
            "sshCommand": format!("ssh -p {} tok-1@127.0.0.1", server.addr.port()),
            "expiresAt": "2099-01-01T00:00:00Z",
        }),
    );
    Ok(running)
}

fn open_request(lease: &str, id: &str) -> TestResult<Vec<u8>> {
    let request = TerminalRequest {
        lease_id: LeaseId::new(lease),
        terminal_id: TerminalId::new(id)?,
        op: TerminalOp::Open {
            cols: TerminalDimension::new(80)?,
            rows: TerminalDimension::new(24)?,
            cmd: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            resume_from: None,
        },
    };
    Ok(serde_json::to_vec(&request)?)
}

fn attach_request(lease: &str) -> TestResult<SshAttachRequest> {
    Ok(SshAttachRequest {
        session_id: SshSessionId::generate(),
        lease_id: LeaseId::new(lease),
        public_key: EphemeralSessionKey::generate()?.public_key_line,
        ttl_seconds: NonZeroU32::new(600).ok_or("zero ttl")?,
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn the_ssh_access_verb_answers_with_the_gateway_access() -> TestResult {
    let server = GatewayServer::start().await?;
    let mut running = running_with(&server).await?;

    let reply = running.verb("ssh_access", LEASE, json!({"ttl_seconds": 600})).await?;

    assert_eq!(reply["outcome"], "available", "{reply}");
    assert_eq!(reply["host"], "127.0.0.1");
    assert_eq!(reply["port"], server.addr.port());
    assert_eq!(reply["user"], "tok-1");
    assert_eq!(reply["expires_at"], 4_070_908_800_u64);
    let calls = running
        .fake
        .calls("POST", &format!("/sandbox/{}/ssh-access", support::SANDBOX));
    assert!(calls.iter().all(|call| call.query.contains("expiresInMinutes=10")));
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_ssh_access_verb_is_silent_for_a_lease_the_worker_does_not_hold() -> TestResult {
    let server = GatewayServer::start().await?;
    let mut running = running_with(&server).await?;

    running
        .silent(
            "sandbox.provider.daytona.ssh_access.lease-9",
            &json!({"lease_id": "lease-9"}),
        )
        .await?;

    assert_eq!(
        running
            .fake
            .count("POST", &format!("/sandbox/{}/ssh-access", support::SANDBOX)),
        0
    );
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_terminal_opens_through_the_worker_and_cancelling_the_lease_closes_it() -> TestResult {
    let server = GatewayServer::start().await?;
    let mut running = running_with(&server).await?;
    let id = TerminalId::new("term-1")?;
    let mut output = running.client.subscribe(terminal_output(&id)).await?;
    let subject = provider_terminal(ProviderKind::Daytona, &LeaseId::new(LEASE));

    let reply = request_until_answered(&running.client, &subject, open_request(LEASE, "term-1")?).await?;

    let opened: TerminalResponse = serde_json::from_slice(&reply.payload)?;
    assert_eq!(opened, TerminalResponse::Opened { terminal_id: id });
    let mut seen = String::new();
    while !seen.contains("ready") {
        let message = output.next().await.ok_or("the output ended")?;
        let frame: TerminalOutput = serde_json::from_slice(&message.payload)?;
        let bytes = terminal_output_bytes(&frame).ok_or("the terminal ended early")?;
        seen.push_str(&String::from_utf8_lossy(&bytes));
    }
    assert_eq!(server.seen().logins, 1);

    running.cancel_lease(LEASE).await?;

    let ended = async {
        loop {
            let message = output.next().await.ok_or("the output ended without an exit frame")?;
            if matches!(serde_json::from_slice(&message.payload)?, TerminalOutput::Exit { .. }) {
                return Ok::<_, gateway_support::Boxed>(());
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), ended).await??;
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_terminal_for_a_lease_the_worker_does_not_hold_gets_no_answer() -> TestResult {
    let server = GatewayServer::start().await?;
    let mut running = running_with(&server).await?;
    let subject = provider_terminal(ProviderKind::Daytona, &LeaseId::new("lease-9"));
    // The handler subscribes in a spawned task; a held lease answering shows it is up.
    let held = provider_terminal(ProviderKind::Daytona, &LeaseId::new(LEASE));
    request_until_answered(&running.client, &held, open_request(LEASE, "warm")?).await?;

    assert!(unanswered(&running.client, &subject, open_request("lease-9", "t")?).await);

    assert_eq!(server.seen().logins, 1, "only the held lease reached the gateway");
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ssh_attach_is_accepted_for_a_held_lease_and_detached_again() -> TestResult {
    let server = GatewayServer::start().await?;
    let mut running = running_with(&server).await?;
    let request = attach_request(LEASE)?;

    let reply = request_until_answered(
        &running.client,
        &ssh_attach(&request.lease_id),
        serde_json::to_vec(&request)?,
    )
    .await?;
    let attached: SshAttachResponse = serde_json::from_slice(&reply.payload)?;
    let detach = SshDetachRequest {
        session_id: request.session_id.clone(),
        lease_id: request.lease_id.clone(),
        reason: "client closed".to_string(),
    };
    let reply = request_until_answered(
        &running.client,
        &ssh_detach(&detach.lease_id),
        serde_json::to_vec(&detach)?,
    )
    .await?;
    let detached: SshDetachResponse = serde_json::from_slice(&reply.payload)?;

    assert_eq!(attached, SshAttachResponse::accepted());
    assert_eq!(detached, SshDetachResponse::accepted());
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ssh_attach_for_a_lease_the_worker_does_not_hold_gets_no_answer() -> TestResult {
    let server = GatewayServer::start().await?;
    let mut running = running_with(&server).await?;
    let held = attach_request(LEASE)?;
    // Wait until the bridge is up by attaching a held lease.
    request_until_answered(&running.client, &ssh_attach(&held.lease_id), serde_json::to_vec(&held)?).await?;
    let other = attach_request("lease-9")?;

    assert!(
        unanswered(
            &running.client,
            &ssh_attach(&other.lease_id),
            serde_json::to_vec(&other)?
        )
        .await
    );

    running.stop().await
}
