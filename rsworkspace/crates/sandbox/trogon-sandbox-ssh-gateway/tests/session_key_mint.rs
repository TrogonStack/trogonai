mod support;

use std::num::NonZeroU32;
use std::time::{Duration, UNIX_EPOCH};

use bytes::Bytes;
use support::{TestResult, json, respond, start_nats};
use trogon_sandbox_contract::{
    CredentialAck, CredentialAckOutcome, CredentialError, CredentialIssueOutcome, CredentialIssueRequest,
    CredentialIssueResponse, CredentialScope, LeaseId, PullSecretId, ServiceAccountId, ShortLivedToken, TenantId,
    TokenId,
};
use trogon_sandbox_credential_client::ClientError;
use trogon_sandbox_ssh_gateway::{SessionCredential, SessionKeyMint, SshSessionId};

const ISSUE: &str = "sandbox.credentials.issue.ssh.tenant-a";
const REVOKE: &str = "sandbox.credentials.revoke.ssh.tenant-a";

fn mint(client: async_nats::Client) -> TestResult<SessionKeyMint> {
    Ok(SessionKeyMint::new(
        client,
        TenantId::new("tenant-a")?,
        PullSecretId::new("pull-1"),
    ))
}

fn credential() -> TestResult<SessionCredential> {
    Ok(SessionCredential {
        token_id: TokenId::new("tok-1"),
        lease_id: LeaseId::new("lease-1"),
        session_id: "s1".parse::<SshSessionId>()?,
    })
}

async fn register(mint: &SessionKeyMint) -> Result<SessionCredential, ClientError> {
    let session: SshSessionId = SshSessionId::generate();
    mint.register_session(
        &session,
        &LeaseId::new("lease-1"),
        NonZeroU32::MIN.saturating_add(59),
        ServiceAccountId::new("sa-1"),
    )
    .await
}

#[tokio::test]
async fn a_session_token_is_issued_for_the_lease() -> TestResult {
    let (_server, client) = start_nats().await?;
    let issued = CredentialIssueResponse {
        token_id: TokenId::new("tok-9"),
        token: ShortLivedToken::new("secret"),
        expires_at: UNIX_EPOCH + Duration::from_secs(100),
    };
    let reply = json(&CredentialIssueOutcome::Ok(issued))?;
    let mut seen = respond(&client, ISSUE, move |_| Some(reply.clone())).await?;
    let credential = register(&mint(client)?).await?;
    assert_eq!(credential.token_id, TokenId::new("tok-9"));
    assert_eq!(credential.lease_id, LeaseId::new("lease-1"));
    let request: CredentialIssueRequest = serde_json::from_slice(&seen.recv().await.ok_or("no request")?.payload)?;
    assert_eq!(request.pull_secret_id, PullSecretId::new("pull-1"));
    assert_eq!(request.requester, Some(ServiceAccountId::new("sa-1")));
    assert_eq!(request.ttl_seconds.get(), 60);
    assert!(matches!(
        request.scope,
        CredentialScope::SshSession { lease_id } if lease_id == LeaseId::new("lease-1")
    ));
    Ok(())
}

#[tokio::test]
async fn a_broker_refusal_fails_the_registration() -> TestResult {
    let (_server, client) = start_nats().await?;
    let refusal = json(&CredentialIssueOutcome::Err(CredentialError::ScopeDenied {
        message: "no".to_string(),
    }))?;
    respond(&client, ISSUE, move |_| Some(refusal.clone())).await?;
    assert!(matches!(register(&mint(client)?).await, Err(ClientError::Broker(_))));
    Ok(())
}

#[tokio::test]
async fn revoking_an_accepted_session_succeeds() -> TestResult {
    let (_server, client) = start_nats().await?;
    let ack = json(&CredentialAckOutcome::Ok(CredentialAck { message: None }))?;
    let mut seen = respond(&client, REVOKE, move |_| Some(ack.clone())).await?;
    mint(client)?.revoke_session(&credential()?, "channel_close").await?;
    assert!(seen.recv().await.is_some());
    Ok(())
}

#[tokio::test]
async fn a_broker_that_refuses_the_revoke_is_tolerated() -> TestResult {
    let (_server, client) = start_nats().await?;
    let refusal = json(&CredentialAckOutcome::Err(CredentialError::ScopeDenied {
        message: "no".to_string(),
    }))?;
    respond(&client, REVOKE, move |_| Some(refusal.clone())).await?;
    mint(client)?.revoke_session(&credential()?, "channel_close").await?;
    Ok(())
}

#[tokio::test]
async fn a_garbled_revoke_reply_is_tolerated() -> TestResult {
    let (_server, client) = start_nats().await?;
    respond(&client, REVOKE, |_| Some(Bytes::from_static(b"nope"))).await?;
    mint(client)?.revoke_session(&credential()?, "channel_close").await?;
    Ok(())
}

#[tokio::test]
async fn an_undeliverable_revoke_is_an_error() -> TestResult {
    let (_server, client) = start_nats().await?;
    let result = mint(client)?.revoke_session(&credential()?, "channel_close").await;
    assert!(matches!(result, Err(ClientError::Transport(_))));
    Ok(())
}
