use std::error::Error;
use std::future::pending;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use tokio::io::AsyncWriteExt;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    CredentialAckOutcome, CredentialError, CredentialId, CredentialIssueOutcome, CredentialIssueRequest,
    CredentialProvider, CredentialRevokeRequest, CredentialRotateRequest, CredentialScope, PullSecretId, TenantId,
};
use trogon_sandbox_credential_broker::{
    BackendError, BrokerService, CredentialBroker, CredentialMaterial, InMemoryAuditLogger, InMemoryCredentialStore,
    InMemoryTokenStore, InProcessKey, ServiceError, TokenRecord, TokenStore, TokenStoreError,
};

const REGISTRY: CredentialProvider = CredentialProvider::Registry;

fn service() -> Arc<BrokerService> {
    let kms = Arc::new(InProcessKey::from_bytes([3u8; 32]));
    Arc::new(BrokerService::new(Arc::new(CredentialBroker::new(
        Arc::new(InMemoryCredentialStore::new(kms)),
        Arc::new(InMemoryTokenStore::new()),
        Arc::new(InMemoryAuditLogger::new()),
    ))))
}

async fn seeded() -> Result<Arc<BrokerService>, Box<dyn Error>> {
    let kms = Arc::new(InProcessKey::from_bytes([3u8; 32]));
    let broker = CredentialBroker::new(
        Arc::new(InMemoryCredentialStore::new(kms)),
        Arc::new(InMemoryTokenStore::new()),
        Arc::new(InMemoryAuditLogger::new()),
    );
    broker
        .store_credential(
            REGISTRY,
            &TenantId::new("tenant-a")?,
            &CredentialId::new("c"),
            CredentialMaterial::RegistryBasic {
                username: "u".to_string(),
                password: "p".to_string(),
            },
        )
        .await?;
    Ok(Arc::new(BrokerService::new(Arc::new(broker))))
}

fn issue_payload(secret: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    Ok(serde_json::to_vec(&CredentialIssueRequest {
        pull_secret_id: PullSecretId::new(secret),
        scope: CredentialScope::RegistryPull {
            repository_glob: "r/*".to_string(),
        },
        ttl_seconds: NonZeroU32::new(60).ok_or("ttl")?,
        requester: None,
        fence: None,
    })?)
}

#[tokio::test]
async fn handlers_reject_malformed_requests() -> Result<(), Box<dyn Error>> {
    let service = service();
    let payload = issue_payload("c")?;
    assert!(matches!(
        service.issue("other.subject", &payload).await,
        Err(ServiceError::UnexpectedSubject)
    ));
    assert!(matches!(
        service.issue("sandbox.credentials.issue.registry", &payload).await,
        Err(ServiceError::MissingTenant)
    ));
    assert!(matches!(
        service.issue("sandbox.credentials.issue.nope.tenant-a", &payload).await,
        Err(ServiceError::Provider(_))
    ));
    assert!(matches!(
        service.issue("sandbox.credentials.issue.registry.", &payload).await,
        Err(ServiceError::Tenant(_))
    ));
    assert!(matches!(
        service.issue("sandbox.credentials.issue.registry.tenant-a", b"{").await,
        Err(ServiceError::Decode(_))
    ));
    assert!(matches!(service.revoke(b"{").await, Err(ServiceError::Decode(_))));
    assert!(matches!(
        service.rotate("other", b"{}").await,
        Err(ServiceError::UnexpectedSubject)
    ));
    assert!(matches!(
        service.rotate("sandbox.credentials.rotate.nope", b"{}").await,
        Err(ServiceError::Provider(_))
    ));
    assert!(matches!(
        service.rotate("sandbox.credentials.rotate.registry", b"{").await,
        Err(ServiceError::Decode(_))
    ));
    Ok(())
}

#[tokio::test]
async fn a_broker_refusal_is_part_of_the_reply() -> Result<(), Box<dyn Error>> {
    let service = service();
    let reply = service
        .issue(
            "sandbox.credentials.issue.registry.tenant-a",
            &issue_payload("missing")?,
        )
        .await?;
    let outcome: CredentialIssueOutcome = serde_json::from_slice(&reply)?;
    assert!(matches!(
        outcome,
        CredentialIssueOutcome::Err(CredentialError::NotFound { .. })
    ));
    let revoke = service
        .revoke(&serde_json::to_vec(&CredentialRevokeRequest {
            token_id: trogon_sandbox_contract::TokenId::new("nope"),
            reason: String::new(),
        })?)
        .await?;
    assert!(matches!(
        serde_json::from_slice::<CredentialAckOutcome>(&revoke)?,
        CredentialAckOutcome::Ok(_)
    ));
    let rotate = service
        .rotate(
            "sandbox.credentials.rotate.registry.",
            &serde_json::to_vec(&CredentialRotateRequest {
                credential_id: CredentialId::new("missing"),
            })?,
        )
        .await?;
    assert!(matches!(
        serde_json::from_slice::<CredentialAckOutcome>(&rotate)?,
        CredentialAckOutcome::Err(_)
    ));
    Ok(())
}

struct RefusingTokens;

#[async_trait::async_trait]
impl TokenStore for RefusingTokens {
    async fn issue(
        &self,
        _credential_id: &CredentialId,
        _scope: &CredentialScope,
        _token_value: trogon_sandbox_contract::ShortLivedToken,
        _ttl_seconds: NonZeroU32,
    ) -> Result<trogon_sandbox_contract::CredentialIssueResponse, TokenStoreError> {
        Err(BackendError::new(std::io::Error::other("down")).into())
    }

    async fn validate(&self, _token_id: &trogon_sandbox_contract::TokenId) -> Result<TokenRecord, CredentialError> {
        Err(CredentialError::Backend {
            message: "down".to_string(),
        })
    }

    async fn revoke(&self, _token_id: &trogon_sandbox_contract::TokenId) -> Result<(), TokenStoreError> {
        Err(BackendError::new(std::io::Error::other("down")).into())
    }
}

#[tokio::test]
async fn a_failed_revocation_is_part_of_the_reply() -> Result<(), Box<dyn Error>> {
    let kms = Arc::new(InProcessKey::from_bytes([3u8; 32]));
    let service = BrokerService::new(Arc::new(CredentialBroker::new(
        Arc::new(InMemoryCredentialStore::new(kms)),
        Arc::new(RefusingTokens),
        Arc::new(InMemoryAuditLogger::new()),
    )));
    let reply = service
        .revoke(&serde_json::to_vec(&CredentialRevokeRequest {
            token_id: trogon_sandbox_contract::TokenId::new("t"),
            reason: String::new(),
        })?)
        .await?;
    assert!(matches!(
        serde_json::from_slice::<CredentialAckOutcome>(&reply)?,
        CredentialAckOutcome::Err(_)
    ));
    Ok(())
}

async fn ask(client: &async_nats::Client, subject: &str, payload: Vec<u8>) -> Result<Bytes, Box<dyn Error>> {
    let reply = tokio::time::timeout(
        Duration::from_secs(10),
        client.request(subject.to_string(), Bytes::from(payload)),
    )
    .await??;
    Ok(reply.payload)
}

#[tokio::test]
async fn requests_are_answered_over_nats_until_shutdown() -> Result<(), Box<dyn Error>> {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let subscriptions = seeded().await?.subscribe(client.clone()).await?;
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let running = tokio::spawn(subscriptions.run(async move {
        let _ = stopped.await;
    }));

    let issued = ask(
        &client,
        "sandbox.credentials.issue.registry.tenant-a",
        issue_payload("c")?,
    )
    .await?;
    let token_id = match serde_json::from_slice::<CredentialIssueOutcome>(&issued)? {
        CredentialIssueOutcome::Ok(response) => {
            assert_eq!(response.token.expose_secret(), "u:p");
            response.token_id
        }
        CredentialIssueOutcome::Err(error) => return Err(error.to_string().into()),
    };
    let revoked = ask(
        &client,
        "sandbox.credentials.revoke.registry",
        serde_json::to_vec(&CredentialRevokeRequest {
            token_id,
            reason: "done".to_string(),
        })?,
    )
    .await?;
    assert!(matches!(
        serde_json::from_slice::<CredentialAckOutcome>(&revoked)?,
        CredentialAckOutcome::Ok(_)
    ));
    let rotated = ask(
        &client,
        "sandbox.credentials.rotate.registry",
        serde_json::to_vec(&CredentialRotateRequest {
            credential_id: CredentialId::new("c"),
        })?,
    )
    .await?;
    assert!(matches!(
        serde_json::from_slice::<CredentialAckOutcome>(&rotated)?,
        CredentialAckOutcome::Err(_) | CredentialAckOutcome::Ok(_)
    ));

    // A malformed request is logged and gets no reply, and a message without a reply subject is ignored.
    client
        .publish("sandbox.credentials.issue.registry.tenant-a", Bytes::from_static(b"{"))
        .await?;
    client
        .publish(
            "sandbox.credentials.issue.registry.tenant-a",
            Bytes::from(issue_payload("c")?),
        )
        .await?;
    client.flush().await?;
    let malformed = tokio::time::timeout(
        Duration::from_millis(300),
        client.request("sandbox.credentials.issue.registry.tenant-a", Bytes::from_static(b"{")),
    )
    .await;
    assert!(malformed.is_err());

    let _ = stop.send(());
    running.await?;
    Ok(())
}

#[tokio::test]
async fn a_reply_that_cannot_be_published_is_dropped() -> Result<(), Box<dyn Error>> {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let subscriptions = seeded().await?.subscribe(client.clone()).await?;
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let running = tokio::spawn(subscriptions.run(async move {
        let _ = stopped.await;
    }));
    // A raw connection can name a reply subject the client library refuses to publish to.
    let address = server.address().trim_start_matches("nats://").to_string();
    let mut raw = tokio::net::TcpStream::connect(address).await?;
    let payload = issue_payload("c")?;
    let mut frame = b"CONNECT {}\r\n".to_vec();
    frame.extend_from_slice(
        format!(
            "PUB sandbox.credentials.issue.registry.tenant-a a..b {}\r\n",
            payload.len()
        )
        .as_bytes(),
    );
    frame.extend_from_slice(&payload);
    frame.extend_from_slice(b"\r\nPING\r\n");
    raw.write_all(&frame).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = stop.send(());
    running.await?;
    Ok(())
}

#[tokio::test]
async fn a_closed_subscription_stream_ends_the_service() -> Result<(), Box<dyn Error>> {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let subscriptions = seeded().await?.subscribe(client.clone()).await?;
    let running = tokio::spawn(subscriptions.run(pending::<()>()));
    client.drain().await?;
    tokio::time::timeout(Duration::from_secs(10), running).await??;
    Ok(())
}

#[tokio::test]
async fn subscribing_on_a_drained_client_fails() -> Result<(), Box<dyn Error>> {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    client.drain().await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(seeded().await?.subscribe(client).await.is_err());
    Ok(())
}
