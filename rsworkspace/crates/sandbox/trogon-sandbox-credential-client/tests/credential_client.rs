use std::num::NonZeroU32;
use std::time::Duration;

use bytes::Bytes;
use trogon_nats::AdvancedMockNatsClient;
use trogon_sandbox_contract::{
    CredentialAck, CredentialAckOutcome, CredentialError, CredentialId, CredentialIssueOutcome, CredentialIssueRequest,
    CredentialIssueResponse, CredentialProvider, CredentialRevokeRequest, CredentialRotateRequest, CredentialScope,
    LeaseId, PolicyFence, PullSecretId, ServiceAccountId, ShortLivedToken, TenantId, TokenId,
};
use trogon_sandbox_credential_client::{ClientError, CredentialClient, IssueParams};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ISSUE_SUBJECT: &str = "sandbox.credentials.issue.registry.acme";
const REVOKE_SUBJECT: &str = "sandbox.credentials.revoke.registry.acme";
const ROTATE_SUBJECT: &str = "sandbox.credentials.rotate.registry";

fn client(
    mock: &AdvancedMockNatsClient,
) -> Result<CredentialClient<AdvancedMockNatsClient>, Box<dyn std::error::Error>> {
    Ok(CredentialClient::new(
        mock.clone(),
        CredentialProvider::Registry,
        TenantId::new("acme")?,
    ))
}

fn params() -> Result<IssueParams, Box<dyn std::error::Error>> {
    Ok(IssueParams {
        pull_secret_id: PullSecretId::new("pull-1"),
        scope: CredentialScope::RegistryPull {
            repository_glob: "acme/*".to_owned(),
        },
        ttl_seconds: NonZeroU32::new(60).ok_or("zero ttl")?,
        requester: Some(ServiceAccountId::new("svc")),
    })
}

fn reply(value: &impl serde::Serialize) -> Result<Bytes, serde_json::Error> {
    Ok(Bytes::from(serde_json::to_vec(value)?))
}

#[tokio::test]
async fn issue_returns_the_token_and_sends_the_request_without_a_fence() -> TestResult {
    let mock = AdvancedMockNatsClient::new();
    let response = CredentialIssueResponse {
        token_id: TokenId::new("tok-1"),
        token: ShortLivedToken::new("secret"),
        expires_at: std::time::UNIX_EPOCH + Duration::from_secs(100),
    };
    mock.set_response(ISSUE_SUBJECT, reply(&CredentialIssueOutcome::Ok(response.clone()))?);

    let issued = client(&mock)?.issue(params()?).await?;

    assert_eq!(issued, response);
    let sent: CredentialIssueRequest = serde_json::from_slice(&mock.requested_payloads()[0])?;
    assert_eq!(sent.pull_secret_id, PullSecretId::new("pull-1"));
    assert_eq!(sent.ttl_seconds.get(), 60);
    assert_eq!(sent.requester, Some(ServiceAccountId::new("svc")));
    assert_eq!(sent.fence, None);
    Ok(())
}

#[tokio::test]
async fn issue_stamps_the_policy_fence() -> TestResult {
    let mock = AdvancedMockNatsClient::new();
    mock.set_response(
        ISSUE_SUBJECT,
        reply(&CredentialIssueOutcome::Ok(CredentialIssueResponse {
            token_id: TokenId::new("tok-1"),
            token: ShortLivedToken::new("secret"),
            expires_at: std::time::UNIX_EPOCH,
        }))?,
    );
    let fence = PolicyFence {
        lease_id: LeaseId::new("lease-1"),
        policy_revision: 7,
    };

    client(&mock)?.with_policy_fence(fence.clone()).issue(params()?).await?;

    let sent: CredentialIssueRequest = serde_json::from_slice(&mock.requested_payloads()[0])?;
    assert_eq!(sent.fence, Some(fence));
    Ok(())
}

#[tokio::test]
async fn issue_surfaces_a_broker_refusal() -> TestResult {
    let mock = AdvancedMockNatsClient::new();
    let refusal = CredentialError::ScopeDenied {
        message: "no".to_owned(),
    };
    mock.set_response(ISSUE_SUBJECT, reply(&CredentialIssueOutcome::Err(refusal.clone()))?);

    let error = client(&mock)?.issue(params()?).await.err();

    assert!(matches!(error, Some(ClientError::Broker(found)) if found == refusal));
    Ok(())
}

#[tokio::test]
async fn revoke_sends_the_token_and_reason() -> TestResult {
    let mock = AdvancedMockNatsClient::new();
    let ack = CredentialAck {
        message: Some("done".to_owned()),
    };
    mock.set_response(REVOKE_SUBJECT, reply(&CredentialAckOutcome::Ok(ack.clone()))?);

    let received = client(&mock)?.revoke(TokenId::new("tok-1"), "leaked").await?;

    assert_eq!(received, ack);
    let sent: CredentialRevokeRequest = serde_json::from_slice(&mock.requested_payloads()[0])?;
    assert_eq!(sent.token_id, TokenId::new("tok-1"));
    assert_eq!(sent.reason, "leaked");
    Ok(())
}

#[tokio::test]
async fn revoke_surfaces_a_broker_refusal() -> TestResult {
    let mock = AdvancedMockNatsClient::new();
    let refusal = CredentialError::TokenRevoked {
        token_id: TokenId::new("tok-1"),
    };
    mock.set_response(REVOKE_SUBJECT, reply(&CredentialAckOutcome::Err(refusal.clone()))?);

    let error = client(&mock)?.revoke(TokenId::new("tok-1"), "").await.err();

    assert!(matches!(error, Some(ClientError::Broker(found)) if found == refusal));
    Ok(())
}

#[tokio::test]
async fn rotate_uses_the_subject_without_a_tenant() -> TestResult {
    let mock = AdvancedMockNatsClient::new();
    mock.set_response(
        ROTATE_SUBJECT,
        reply(&CredentialAckOutcome::Ok(CredentialAck { message: None }))?,
    );

    let ack = client(&mock)?.rotate(CredentialId::new("cred-1")).await?;

    assert_eq!(ack, CredentialAck { message: None });
    let sent: CredentialRotateRequest = serde_json::from_slice(&mock.requested_payloads()[0])?;
    assert_eq!(sent.credential_id, CredentialId::new("cred-1"));
    Ok(())
}

#[tokio::test]
async fn a_failed_request_is_a_transport_error() -> TestResult {
    let mock = AdvancedMockNatsClient::new();
    mock.fail_next_request();

    let error = client(&mock)?.rotate(CredentialId::new("cred-1")).await.err();

    assert!(matches!(&error, Some(ClientError::Transport(_))));
    assert!(!format!("{}", error.ok_or("no error")?).is_empty());
    Ok(())
}

#[tokio::test]
async fn a_silent_broker_times_out() -> TestResult {
    let mock = AdvancedMockNatsClient::new();
    mock.hang_next_request();
    let timeout = Duration::from_millis(20);

    let error = client(&mock)?
        .with_timeout(timeout)
        .rotate(CredentialId::new("c"))
        .await
        .err();

    assert!(matches!(error, Some(ClientError::Timeout(found)) if found == timeout));
    Ok(())
}

#[tokio::test]
async fn a_malformed_reply_is_a_decode_error() -> TestResult {
    let mock = AdvancedMockNatsClient::new();
    mock.set_response(ROTATE_SUBJECT, Bytes::from_static(b"not json"));

    let error = client(&mock)?.rotate(CredentialId::new("c")).await.err();

    assert!(matches!(error, Some(ClientError::Decode(_))));
    Ok(())
}

#[test]
fn display_names_each_failure() {
    let timeout = ClientError::Timeout(Duration::from_secs(1));
    assert!(!format!("{timeout}").is_empty());
    let broker = ClientError::Broker(CredentialError::Backend {
        message: "x".to_owned(),
    });
    assert!(!format!("{broker}").is_empty());
}

#[test]
fn encode_errors_are_named_in_the_message() -> TestResult {
    let source = serde_json::from_str::<u8>("x").err().ok_or("parsed")?;
    let error = ClientError::Encode(source);
    assert!(!format!("{error}").is_empty());
    Ok(())
}
