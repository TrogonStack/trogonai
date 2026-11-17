//! The secrets service started against a real NATS server and a stand-in for
//! OpenBao's AppRole login: what it refuses to start on, that it serves
//! releases until it is told to stop, and that it announces a revocation to the
//! credential's holder while it does.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use async_nats::header::NATS_MESSAGE_ID;
use async_nats::jetstream::stream;
use async_nats::{HeaderMap, RequestErrorKind};
use axum::Router;
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::routing::post;
use buffa::Message as _;
use bytes::Bytes;
use nkeys::{KeyPair, XKey};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use trogon_credential_aggregate::proto::revoked_to_proto;
use trogon_credential_aggregate::{CREDENTIAL_EVENT_SUBJECT_PREFIX, CREDENTIAL_STREAM, credential_event_subject};
use trogon_credential_domain::{
    CredentialIdentity, CredentialKind, CredentialOwnerId, CredentialRef, CredentialScope, CredentialVersion,
    SourceKind,
};
use trogon_decider_nats::TROGON_EVENT_TYPE;
use trogon_decider_runtime::{EventEncode, EventType as _};
use trogon_nats::ConnectError;
use trogon_nats::jetstream::is_get_stream_not_found;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_secrets_service::{
    AdmissionLimit, ConfigSetting, INVALIDATION_PUBLISHER_DURABLE, INVALIDATION_STREAM, InvalidationStream,
    InvalidationStreamMismatchError, ReleaseSources, SecretsService, SecretsServiceError, ServiceArgs, ServiceConfig,
};
use trogon_std::env::InMemoryEnv;
use trogon_std::fs::MemFs;
use trogon_std::log_capture::{CapturedEvents, LevelFilter};
use trogonai_proto::gateway::credentials::v1;
use trogonai_proto::secrets::exchange::v1alpha1::{CredentialAddress, ReleaseInvalidated, ReleaseInvalidationReason};
use uuid::Uuid;

const TENANT_OWNERS_FILE: &str = "/etc/trogon/tenant-owners.toml";
const RELEASE_POLICIES_FILE: &str = "/etc/trogon/release-policies.toml";
const SECRET_ID_FILE: &str = "/run/secrets/fake-secret-id";
/// The credential owner every test tenant holds.
const OWNER: &str = "project-1";
const SEED_VARIABLE: &str = "FAKE_SERVICE_XKEY_SEED";
const APPROLE_LOGIN: &str = "/v1/auth/approle/login";
const LOGIN_ANSWER: &str = r#"{"auth":{"client_token":"fake-login-token","lease_duration":3600,"renewable":true}}"#;
/// What a sealed OpenBao answers a login with.
const SEALED: &str = "fake-sealed";
/// A NATS server address nothing listens on.
const UNREACHABLE_NATS: &str = "127.0.0.1:1";
/// A NATS credentials file no deployment provisioned.
const MISSING_CREDS_FILE: &str = "/nonexistent/trogon-secrets-service/fake.creds";
const RELEASE_POLICIES: &str = r#"
[[policies]]
owner_id = "project-1"
credential_id = "0190a5b4-7c1e-7d2a-9f3b-6c8d4e2a1b0f"
runtime_services = ["billing-runner"]
hosts = ["api.github.com"]
headers = ["authorization"]
"#;
const SERVING_WITHIN: Duration = Duration::from_secs(10);
/// Long enough for the credential stream lookup over a connection that never
/// reaches a server to give up.
const REFUSED_WITHIN: Duration = Duration::from_secs(30);
const STOPPED_WITHIN: Duration = Duration::from_secs(10);
const SERVING: &str = "the secrets service is serving releases";

async fn serve_openbao(app: Router) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let serving = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (address, serving)
}

/// Answers OpenBao's AppRole login with a token and nothing else.
async fn fake_openbao() -> (String, JoinHandle<()>) {
    serve_openbao(Router::new().route(
        APPROLE_LOGIN,
        post(|| async { ([(CONTENT_TYPE, "application/json")], LOGIN_ANSWER) }),
    ))
    .await
}

/// Refuses every AppRole login the way a sealed OpenBao does, and signals
/// each attempt on the returned [`Notify`].
async fn sealed_openbao() -> (String, Arc<Notify>, JoinHandle<()>) {
    let attempted = Arc::new(Notify::new());
    let signal = Arc::clone(&attempted);
    let (address, serving) = serve_openbao(Router::new().route(
        APPROLE_LOGIN,
        post(move || {
            signal.notify_one();
            async { (StatusCode::SERVICE_UNAVAILABLE, SEALED) }
        }),
    ))
    .await;
    (address, attempted, serving)
}

async fn create_credential_stream(server: &JetStreamTestServer) {
    server
        .jetstream()
        .await
        .create_stream(stream::Config {
            name: CREDENTIAL_STREAM.to_owned(),
            subjects: vec![format!("{CREDENTIAL_EVENT_SUBJECT_PREFIX}.>")],
            ..Default::default()
        })
        .await
        .unwrap();
}

/// Provisions the invalidation stream the way an operator would, since the
/// service only checks it.
async fn create_invalidation_stream(server: &JetStreamTestServer, config: stream::Config) {
    server.jetstream().await.create_stream(config).await.unwrap();
}

/// The streams a service needs before it takes a release.
async fn create_streams(server: &JetStreamTestServer) {
    create_credential_stream(server).await;
    create_invalidation_stream(server, InvalidationStream::config(NonZeroUsize::MIN)).await;
}

/// The environment the service is deployed with: NATS at `nats` without TLS,
/// OpenBao at `openbao` without TLS, the sealing key seed in a variable, and
/// the single replica a one-node test server can give the invalidation stream.
fn environment(nats: &str, openbao: &str) -> InMemoryEnv {
    let env = InMemoryEnv::new();
    for (setting, value) in [
        (ConfigSetting::NatsUrl, nats),
        (ConfigSetting::NatsTlsDisabled, "true"),
        (ConfigSetting::OpenBaoAddress, openbao),
        (ConfigSetting::OpenBaoTlsDisabled, "true"),
        (ConfigSetting::OpenBaoMount, "secret"),
        (ConfigSetting::OpenBaoRoleId, "fake-role-id"),
        (ConfigSetting::OpenBaoSecretIdFile, SECRET_ID_FILE),
        (ConfigSetting::OpenBaoChildTokenRole, "secrets-release"),
        (ConfigSetting::OpenBaoChildTokenPolicy, "secrets-release-read"),
        (ConfigSetting::TenantOwnersFile, TENANT_OWNERS_FILE),
        (ConfigSetting::ReleasePoliciesFile, RELEASE_POLICIES_FILE),
        (ConfigSetting::XkeySeed, &format!("env://{SEED_VARIABLE}")),
        (ConfigSetting::AuditTenant, "acme"),
        (ConfigSetting::AuditProject, "platform"),
        (ConfigSetting::AuditInstance, "secrets-0"),
        (ConfigSetting::InvalidationStreamReplicas, "1"),
    ] {
        env.set(setting.env(), value);
    }
    env.set(SEED_VARIABLE, XKey::new().seed().unwrap());
    env
}

fn files(tenant: &str) -> MemFs {
    let fs = MemFs::new();
    fs.insert(TENANT_OWNERS_FILE, format!("[tenants]\n\"{tenant}\" = [\"{OWNER}\"]\n"));
    fs.insert(RELEASE_POLICIES_FILE, RELEASE_POLICIES);
    fs.insert(SECRET_ID_FILE, "fake-secret-id\n");
    fs
}

/// A release subject any tenant's `billing-runner` could call.
fn release_subject() -> String {
    format!(
        "{}.secrets.v1.service.billing-runner.SecretExchangeService.ResolveForRelease",
        KeyPair::new_account().public_key()
    )
}

fn service(nats: &str, openbao: &str) -> SecretsService {
    service_in(&environment(nats, openbao))
}

fn service_in(env: &InMemoryEnv) -> SecretsService {
    service_for(env, &KeyPair::new_account().public_key())
}

/// The service deployed with `tenant` holding [`OWNER`].
fn service_for(env: &InMemoryEnv, tenant: &str) -> SecretsService {
    let fs = files(tenant);
    let config = ServiceConfig::resolve(&ServiceArgs::default(), env, &fs).unwrap();
    let sources = ReleaseSources::load(&config, env, &fs).unwrap();
    SecretsService::new(config, sources)
}

/// Runs the service until it returns, failing the test if it is still running
/// after [`REFUSED_WITHIN`].
async fn refusal(service: SecretsService) -> SecretsServiceError {
    tokio::time::timeout(REFUSED_WITHIN, service.run(CancellationToken::new()))
        .await
        .expect("the service started instead of refusing to")
        .unwrap_err()
}

#[tokio::test]
async fn nats_settings_the_connection_cannot_use_refuse_to_start() {
    let (openbao, _openbao) = fake_openbao().await;
    let env = environment(UNREACHABLE_NATS, &openbao);
    env.set(ConfigSetting::NatsCreds.env(), MISSING_CREDS_FILE);

    let refused = refusal(service_in(&env)).await;

    assert!(
        matches!(
            refused,
            SecretsServiceError::Connect(ConnectError::InvalidCredentials(_))
        ),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_nats_server_that_cannot_be_reached_refuses_to_start() {
    let (openbao, _openbao) = fake_openbao().await;

    let refused = refusal(service(UNREACHABLE_NATS, &openbao)).await;

    assert!(
        matches!(refused, SecretsServiceError::CredentialStream { .. }),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_service_told_to_stop_while_nats_cannot_be_reached_stops_cleanly() {
    let (openbao, _openbao) = fake_openbao().await;
    let shutdown = CancellationToken::new();
    shutdown.cancel();

    tokio::time::timeout(STOPPED_WITHIN, service(UNREACHABLE_NATS, &openbao).run(shutdown))
        .await
        .expect("the service waited for the credential stream lookup instead of stopping")
        .unwrap();
}

#[tokio::test]
async fn a_credential_stream_that_does_not_exist_refuses_to_start() {
    let server = JetStreamTestServer::start().await;
    let (openbao, _openbao) = fake_openbao().await;

    let refused = refusal(service(server.address(), &openbao)).await;

    let SecretsServiceError::CredentialStream { stream, .. } = refused else {
        panic!("a missing credential stream was refused as {refused:?}");
    };
    assert_eq!(stream.as_str(), CREDENTIAL_STREAM);
}

#[tokio::test]
async fn an_invalidation_stream_that_does_not_exist_refuses_to_start_without_creating_it() {
    let server = JetStreamTestServer::start().await;
    create_credential_stream(&server).await;
    let (openbao, _openbao) = fake_openbao().await;

    let refused = refusal(service(server.address(), &openbao)).await;

    let SecretsServiceError::InvalidationStream(source) = refused else {
        panic!("a missing invalidation stream was refused as {refused:?}");
    };
    assert!(is_get_stream_not_found(&source), "{source:?}");
    let lookup = server.jetstream().await.get_stream(INVALIDATION_STREAM).await;
    assert!(
        lookup.as_ref().is_err_and(is_get_stream_not_found),
        "the service created the invalidation stream it only checks"
    );
}

#[tokio::test]
async fn an_invalidation_stream_that_lets_a_notice_be_deleted_refuses_to_start() {
    let server = JetStreamTestServer::start().await;
    create_credential_stream(&server).await;
    create_invalidation_stream(
        &server,
        stream::Config {
            deny_delete: false,
            ..InvalidationStream::config(NonZeroUsize::MIN)
        },
    )
    .await;
    let (openbao, _openbao) = fake_openbao().await;

    let refused = refusal(service(server.address(), &openbao)).await;

    assert!(
        matches!(
            refused,
            SecretsServiceError::InvalidationStreamMismatch(InvalidationStreamMismatchError::DenyDelete)
        ),
        "{refused:?}"
    );
}

#[tokio::test]
async fn an_invalidation_stream_with_fewer_replicas_than_configured_refuses_to_start() {
    let server = JetStreamTestServer::start().await;
    create_streams(&server).await;
    let (openbao, _openbao) = fake_openbao().await;
    let env = environment(server.address(), &openbao);
    env.set(ConfigSetting::InvalidationStreamReplicas.env(), "3");

    let refused = refusal(service_in(&env)).await;

    assert!(
        matches!(
            refused,
            SecretsServiceError::InvalidationStreamMismatch(InvalidationStreamMismatchError::Replicas)
        ),
        "{refused:?}"
    );
}

#[tokio::test]
async fn an_openbao_that_refuses_the_login_keeps_the_service_out_of_the_queue_group_until_it_stops() {
    let server = JetStreamTestServer::start().await;
    create_streams(&server).await;
    let (openbao, attempted, _openbao) = sealed_openbao().await;
    let shutdown = CancellationToken::new();
    let running = tokio::spawn(service(server.address(), &openbao).run(shutdown.clone()));

    tokio::time::timeout(SERVING_WITHIN, attempted.notified())
        .await
        .expect("the service never tried to log in to OpenBao");
    let unanswered = server
        .client()
        .await
        .request(release_subject(), Bytes::new())
        .await
        .expect_err("the service joined the queue group before OpenBao accepted its login");
    assert_eq!(unanswered.kind(), RequestErrorKind::NoResponders);
    assert!(
        !running.is_finished(),
        "the service gave up instead of retrying the login"
    );
    shutdown.cancel();

    tokio::time::timeout(STOPPED_WITHIN, running)
        .await
        .expect("the service did not stop while it retried the login")
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn the_service_serves_releases_until_it_is_told_to_stop() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::INFO);
    let server = JetStreamTestServer::start().await;
    create_streams(&server).await;
    let (openbao, _openbao) = fake_openbao().await;
    let shutdown = CancellationToken::new();
    let running = tokio::spawn(service(server.address(), &openbao).run(shutdown.clone()));
    let caller = server.client().await;
    let subject = release_subject();

    tokio::time::timeout(SERVING_WITHIN, async {
        loop {
            match caller.request(subject.clone(), Bytes::new()).await {
                Ok(_) => return,
                Err(error) if error.kind() == RequestErrorKind::NoResponders => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(error) => panic!("the release endpoint did not answer: {error}"),
            }
        }
    })
    .await
    .expect("the service did not start serving releases");
    shutdown.cancel();

    tokio::time::timeout(STOPPED_WITHIN, running)
        .await
        .expect("the service did not stop when told to")
        .unwrap()
        .unwrap();
    let serving = events
        .events()
        .into_iter()
        .find(|event| event.message() == Some(SERVING))
        .expect("the service did not log that it serves releases");
    assert_eq!(serving.field("credential_stream"), Some(CREDENTIAL_STREAM));
    assert_eq!(
        serving.field("admission_limit"),
        Some(AdmissionLimit::default().get().to_string().as_str())
    );
}

#[tokio::test]
async fn a_service_told_to_stop_before_it_serves_stops_cleanly() {
    let server = JetStreamTestServer::start().await;
    create_streams(&server).await;
    let (openbao, _openbao) = fake_openbao().await;
    let shutdown = CancellationToken::new();
    shutdown.cancel();

    tokio::time::timeout(STOPPED_WITHIN, service(server.address(), &openbao).run(shutdown))
        .await
        .expect("the service did not stop when told to")
        .unwrap();
}

#[tokio::test]
async fn a_revocation_appended_while_the_service_runs_reaches_its_holder_as_one_notice() {
    let server = JetStreamTestServer::start().await;
    create_streams(&server).await;
    let (openbao, _openbao) = fake_openbao().await;
    let tenant = KeyPair::new_account().public_key();
    let shutdown = CancellationToken::new();
    let running = tokio::spawn(service_for(&environment(server.address(), &openbao), &tenant).run(shutdown.clone()));
    let jetstream = server.jetstream().await;
    let credentials = jetstream.get_stream(CREDENTIAL_STREAM).await.unwrap();
    // The publisher reads only the events appended after its durable exists.
    tokio::time::timeout(SERVING_WITHIN, async {
        while credentials.consumer_info(INVALIDATION_PUBLISHER_DURABLE).await.is_err() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the service never started reading the credential stream");
    let identity = CredentialIdentity::derive(
        CredentialScope::source(CredentialOwnerId::new(OWNER).unwrap(), SourceKind::GitHub),
        CredentialKind::WebhookSecret,
    );
    let revoked = v1::CredentialEvent {
        event: Some(
            revoked_to_proto(&CredentialRef::new(
                identity.clone(),
                CredentialVersion::new(1).unwrap(),
            ))
            .into(),
        ),
    };
    // Appended the way the credential store appends, which every event it
    // records carries.
    let mut headers = HeaderMap::new();
    headers.insert(NATS_MESSAGE_ID, Uuid::new_v4().to_string());
    headers.insert(TROGON_EVENT_TYPE, revoked.event_type().unwrap());
    let event_subject = credential_event_subject(&identity.key()).unwrap();
    let appended = jetstream
        .publish_with_headers(
            event_subject.as_str().to_owned(),
            headers,
            EventEncode::encode(&revoked).unwrap().into(),
        )
        .await
        .unwrap()
        .await
        .unwrap();
    let mut invalidations = jetstream.get_stream(INVALIDATION_STREAM).await.unwrap();
    let notice_subject = format!("secrets.invalidation.v1.{tenant}.{OWNER}.{}", identity.id());

    let notice = tokio::time::timeout(SERVING_WITHIN, async {
        loop {
            match invalidations.get_last_raw_message_by_subject(&notice_subject).await {
                Ok(notice) => return notice,
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
    })
    .await
    .expect("the revocation never reached its holder");
    shutdown.cancel();
    tokio::time::timeout(STOPPED_WITHIN, running)
        .await
        .expect("the service did not stop when told to")
        .unwrap()
        .unwrap();

    // Destructured without `..`, so a field added to the contract stops this
    // test compiling until someone decides whether it could hold a value.
    let ReleaseInvalidated {
        credential,
        reason,
        occurred_at,
        source_sequence,
    } = ReleaseInvalidated::decode_from_slice(&notice.payload).unwrap();
    let CredentialAddress {
        credential_id,
        owner_id,
    } = credential.into_option().unwrap();
    assert_eq!(credential_id, identity.id().to_string());
    assert_eq!(owner_id, OWNER);
    assert_eq!(
        reason.as_known(),
        Some(ReleaseInvalidationReason::RELEASE_INVALIDATION_REASON_REVOKED)
    );
    let stored = credentials.get_raw_message(appended.sequence).await.unwrap();
    let occurred_at = occurred_at.into_option().unwrap();
    assert_eq!(
        (occurred_at.seconds, occurred_at.nanos),
        (
            stored.time.unix_timestamp(),
            i32::try_from(stored.time.nanosecond()).unwrap()
        )
    );
    assert_eq!(source_sequence, appended.sequence);
    assert_eq!(invalidations.info().await.unwrap().state.messages, 1);
}
