mod support;

use bytes::Bytes;
use support::*;
use trogon_nats::AdvancedMockNatsClient;
use trogon_sandbox_contract::{
    CredentialIssueOutcome, CredentialIssueResponse, CredentialScope, ShortLivedToken, TenantId, TokenId,
};
use trogon_sandbox_modal_worker::{BrokeredVolumeStores, VolumeSettings, VolumeStores, WorkspaceError};
use trogon_sandbox_volumes::VolumeEndpoint;

const ISSUE_SUBJECT: &str = "sandbox.credentials.issue.s3.acme";

fn issued(token: &str) -> Boxed<Bytes> {
    Ok(Bytes::from(serde_json::to_vec(&CredentialIssueOutcome::Ok(
        CredentialIssueResponse {
            token_id: TokenId::new("tok-1"),
            token: ShortLivedToken::new(token),
            expires_at: std::time::UNIX_EPOCH,
        },
    ))?))
}

fn read_only() -> CredentialScope {
    CredentialScope::S3ReadOnly {
        bucket: "my-bucket".to_owned(),
        prefix: "data".to_owned(),
    }
}

fn stores(mock: &AdvancedMockNatsClient, settings: VolumeSettings) -> BrokeredVolumeStores<AdvancedMockNatsClient> {
    BrokeredVolumeStores::new(mock.clone(), settings)
}

#[tokio::test]
async fn a_credential_with_a_session_opens_a_store() -> TestResult {
    let mock = AdvancedMockNatsClient::new();
    mock.set_response(ISSUE_SUBJECT, issued("access:secret:session")?);
    let store = stores(&mock, volume_settings()?)
        .open(&TenantId::new("acme")?, &read_only())
        .await?;
    assert!(store.to_string().contains("my-bucket"));
    assert_eq!(mock.requested_payloads().len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_credential_without_a_session_opens_a_store_at_a_custom_endpoint() -> TestResult {
    let mock = AdvancedMockNatsClient::new();
    mock.set_response(ISSUE_SUBJECT, issued("access:secret")?);
    let tenant = TenantId::new("acme")?;
    let mut settings = volume_settings()?;
    settings.endpoint = Some(VolumeEndpoint::new("http://127.0.0.1:9000")?);
    stores(&mock, settings.clone()).open(&tenant, &read_only()).await?;
    settings.endpoint = Some(VolumeEndpoint::new("https://s3.example.com")?);
    let write = CredentialScope::S3ReadWrite {
        bucket: "my-bucket".to_owned(),
        prefix: "data".to_owned(),
    };
    stores(&mock, settings).open(&tenant, &write).await?;
    Ok(())
}

#[tokio::test]
async fn a_token_that_is_not_a_key_pair_is_refused() -> TestResult {
    let mock = AdvancedMockNatsClient::new();
    mock.set_response(ISSUE_SUBJECT, issued("no-separator")?);
    let outcome = stores(&mock, volume_settings()?)
        .open(&TenantId::new("acme")?, &read_only())
        .await;
    assert!(matches!(outcome, Err(WorkspaceError::CredentialToken)));
    Ok(())
}

#[tokio::test]
async fn scopes_that_are_not_s3_are_refused_before_asking_the_broker() -> TestResult {
    let mock = AdvancedMockNatsClient::new();
    let scope = CredentialScope::RegistryPull {
        repository_glob: "acme/*".to_owned(),
    };
    let outcome = stores(&mock, volume_settings()?)
        .open(&TenantId::new("acme")?, &scope)
        .await;
    assert!(matches!(outcome, Err(WorkspaceError::ScopeUnsupported)));
    assert!(mock.requested_payloads().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_broker_that_does_not_answer_fails_the_open() -> TestResult {
    let mock = AdvancedMockNatsClient::new();
    mock.fail_next_request();
    let outcome = stores(&mock, volume_settings()?)
        .open(&TenantId::new("acme")?, &read_only())
        .await;
    assert!(matches!(outcome, Err(WorkspaceError::Credential(_))));
    Ok(())
}
