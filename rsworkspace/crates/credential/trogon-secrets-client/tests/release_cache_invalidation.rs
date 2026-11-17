//! The release cache against a real JetStream server: a notice on
//! `SECRETS_INVALIDATION` drops every copy of the credential it names, even
//! when its payload is not a notice, and leaves every other credential held.
//! Without the stream the client refuses to start, and while the stream is
//! gone it holds nothing until it reads again. A consumer reports how far the
//! server has delivered, which the reader checks after a quiet interval.

#![cfg(feature = "test-support")]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::future::Future;
use std::num::NonZeroU16;
use std::time::{Duration, SystemTime};

use async_nats::jetstream::consumer::{AckPolicy, pull};
use async_nats::jetstream::{self, stream};
use futures::StreamExt;
use nkeys::KeyPair;
use trogon_credential_domain::{
    AuditCorrelationId, CredentialId, CredentialKey, CredentialOwnerId, InjectionLocation, ReleaseHost,
    RuntimeServiceId,
};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_secrets_client::{
    CachedReleaseClient, FakeOutcome, FakeSecretsService, INVALIDATION_STREAM, InvalidationConsumer,
    InvalidationStartError, ReleaseCacheMode, ReleaseClient, ReleaseClientConfig, ReleaseDestination, ReleaseRequest,
    RuntimeService, TenantAccount,
};
use trogon_std::SecretString;
use trogon_std::time::SystemClock;

const OWNER: &str = "project.alpha";
const OWNER_TOKEN: &str = "project~alpha";
const NAMED: &str = "0b6f2f3e-7c1a-4f7e-9d55-3b8a1c2d4e5f";
const OTHER: &str = "1c7e3a4f-8d2b-4a6c-9e01-2f3b4c5d6e7f";
const WAIT: Duration = Duration::from_secs(30);

type Client = CachedReleaseClient<FakeSecretsService, SystemClock>;

fn tenant() -> TenantAccount {
    TenantAccount::parse(&KeyPair::new_account().public_key()).unwrap()
}

fn request(tenant: &TenantAccount, credential_id: &str) -> ReleaseRequest {
    ReleaseRequest::new(
        tenant.clone(),
        CredentialKey::new(
            CredentialOwnerId::new(OWNER).unwrap(),
            CredentialId::new(credential_id).unwrap(),
        ),
        ReleaseDestination::new(
            &ReleaseHost::new("api.github.com").unwrap(),
            NonZeroU16::new(443).unwrap(),
        ),
        InjectionLocation::header("authorization").unwrap(),
        AuditCorrelationId::new("use-1").unwrap(),
    )
}

fn releasing(fake: &FakeSecretsService, times: usize) {
    for _ in 0..times {
        fake.script(FakeOutcome::release(
            SecretString::new("fake-github-secret").unwrap(),
            SystemTime::now() + Duration::from_secs(120),
            7,
        ));
    }
}

/// Creates the stream on every tenant's notices, configured as the secrets
/// service creates it.
async fn create_stream(jetstream: &jetstream::Context) {
    jetstream
        .create_stream(stream::Config {
            name: INVALIDATION_STREAM.to_owned(),
            subjects: vec!["secrets.invalidation.v1.*.>".to_owned()],
            storage: stream::StorageType::File,
            retention: stream::RetentionPolicy::Limits,
            discard: stream::DiscardPolicy::Old,
            max_age: Duration::from_secs(3_600),
            duplicate_window: Duration::from_secs(120),
            max_message_size: 1_024,
            deny_delete: true,
            deny_purge: true,
            allow_direct: true,
            ..Default::default()
        })
        .await
        .unwrap();
}

async fn started(fake: &FakeSecretsService, jetstream: &jetstream::Context) -> Result<Client, InvalidationStartError> {
    let client = ReleaseClient::new(
        fake.clone(),
        SystemClock,
        ReleaseClientConfig::new(
            RuntimeService::new(&RuntimeServiceId::new("billing-runner").unwrap()).unwrap(),
            fake.service_xkey(),
        ),
    );
    CachedReleaseClient::start(client, jetstream.clone()).await
}

/// Waits until `reached` holds, checking every few milliseconds.
async fn until<F, Fut>(mut reached: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    tokio::time::timeout(WAIT, async {
        while !reached().await {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the condition was never reached");
}

#[tokio::test]
async fn refuses_to_start_without_the_invalidation_stream() {
    let server = JetStreamTestServer::start().await;

    let refused = started(&FakeSecretsService::new(), &server.jetstream().await)
        .await
        .err()
        .unwrap();

    assert!(matches!(refused, InvalidationStartError::Stream(_)));
}

#[tokio::test]
async fn drops_the_named_credential_and_keeps_the_others() {
    let server = JetStreamTestServer::start().await;
    let jetstream = server.jetstream().await;
    let tenant = tenant();
    create_stream(&jetstream).await;
    let fake = FakeSecretsService::new();
    releasing(&fake, 3);
    let client = started(&fake, &jetstream).await.unwrap();
    let named = request(&tenant, NAMED);
    let other = request(&tenant, OTHER);
    client.release(&named).await.unwrap();
    client.release(&other).await.unwrap();

    jetstream
        .publish(
            format!("secrets.invalidation.v1.{tenant}.{OWNER_TOKEN}.{NAMED}"),
            "fake-not-a-notice".into(),
        )
        .await
        .unwrap()
        .await
        .unwrap();
    until(|| async {
        client.release(&named).await.unwrap();
        fake.received().len() == 3
    })
    .await;
    client.release(&other).await.unwrap();

    assert_eq!(fake.received().len(), 3);
}

#[tokio::test]
async fn holds_nothing_while_the_stream_is_gone_and_again_once_it_is_back() {
    let server = JetStreamTestServer::start().await;
    let jetstream = server.jetstream().await;
    let tenant = tenant();
    create_stream(&jetstream).await;
    let fake = FakeSecretsService::new();
    releasing(&fake, 4);
    let client = started(&fake, &jetstream).await.unwrap();
    let asked = request(&tenant, NAMED);
    client.release(&asked).await.unwrap();

    jetstream.delete_stream(INVALIDATION_STREAM).await.unwrap();
    until(|| async { client.mode() == ReleaseCacheMode::Uncached }).await;
    client.release(&asked).await.unwrap();
    client.release(&asked).await.unwrap();
    assert_eq!(fake.received().len(), 3);

    create_stream(&jetstream).await;
    until(|| async { client.mode() == ReleaseCacheMode::Caching }).await;
    client.release(&asked).await.unwrap();
    client.release(&asked).await.unwrap();

    assert_eq!(fake.received().len(), 4);
}

#[tokio::test]
async fn a_consumer_reports_the_last_notice_the_server_delivered_to_it() {
    let server = JetStreamTestServer::start().await;
    let jetstream = server.jetstream().await;
    let tenant = tenant();
    create_stream(&jetstream).await;
    let consumer = jetstream
        .get_stream(INVALIDATION_STREAM)
        .await
        .unwrap()
        .create_consumer(pull::Config {
            ack_policy: AckPolicy::None,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(consumer.delivered().await.unwrap(), 0);

    jetstream
        .publish(
            format!("secrets.invalidation.v1.{tenant}.{OWNER_TOKEN}.{NAMED}"),
            "fake-notice".into(),
        )
        .await
        .unwrap()
        .await
        .unwrap();
    let mut notices = InvalidationConsumer::messages(&consumer).await.unwrap();
    notices.next().await.unwrap().unwrap();

    assert_eq!(consumer.delivered().await.unwrap(), 1);
}
