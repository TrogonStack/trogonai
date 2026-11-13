//! The secrets service started against a real NATS server and a stand-in for
//! OpenBao's AppRole login: what it refuses to start on, and that it serves
//! releases until it is told to stop.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::time::Duration;

use async_nats::RequestErrorKind;
use async_nats::jetstream::stream;
use axum::Router;
use axum::http::header::CONTENT_TYPE;
use axum::routing::post;
use bytes::Bytes;
use nkeys::{KeyPair, XKey};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use trogon_credential_aggregate::{CREDENTIAL_EVENT_SUBJECT_PREFIX, CREDENTIAL_STREAM};
use trogon_nats::ConnectError;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_secrets_service::{
    ConfigSetting, ReleaseSources, SecretsService, SecretsServiceError, ServiceArgs, ServiceConfig,
};
use trogon_std::env::InMemoryEnv;
use trogon_std::fs::MemFs;

const TENANT_OWNERS_FILE: &str = "/etc/trogon/tenant-owners.toml";
const RELEASE_POLICIES_FILE: &str = "/etc/trogon/release-policies.toml";
const SECRET_ID_FILE: &str = "/run/secrets/fake-secret-id";
const SEED_VARIABLE: &str = "FAKE_SERVICE_XKEY_SEED";
const APPROLE_LOGIN: &str = "/v1/auth/approle/login";
const LOGIN_ANSWER: &str = r#"{"auth":{"client_token":"fake-login-token","lease_duration":3600,"renewable":true}}"#;
/// An address nothing listens on.
const UNREACHABLE_OPENBAO: &str = "http://127.0.0.1:1";
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

/// Answers OpenBao's AppRole login with a token and nothing else.
async fn fake_openbao() -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().route(
        APPROLE_LOGIN,
        post(|| async { ([(CONTENT_TYPE, "application/json")], LOGIN_ANSWER) }),
    );
    let serving = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (address, serving)
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

/// The environment the service is deployed with: NATS at `nats` without TLS,
/// OpenBao at `openbao` without TLS, and the sealing key seed in a variable.
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
    ] {
        env.set(setting.env(), value);
    }
    env.set(SEED_VARIABLE, XKey::new().seed().unwrap());
    env
}

fn files() -> MemFs {
    let fs = MemFs::new();
    fs.insert(
        TENANT_OWNERS_FILE,
        format!(
            "[tenants]\n\"{}\" = [\"project-1\"]\n",
            KeyPair::new_account().public_key()
        ),
    );
    fs.insert(RELEASE_POLICIES_FILE, RELEASE_POLICIES);
    fs.insert(SECRET_ID_FILE, "fake-secret-id\n");
    fs
}

fn service(nats: &str, openbao: &str) -> SecretsService {
    service_in(&environment(nats, openbao))
}

fn service_in(env: &InMemoryEnv) -> SecretsService {
    let fs = files();
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
async fn an_openbao_that_cannot_be_reached_refuses_to_start() {
    let server = JetStreamTestServer::start().await;
    create_credential_stream(&server).await;

    let refused = refusal(service(server.address(), UNREACHABLE_OPENBAO)).await;

    assert!(matches!(refused, SecretsServiceError::OpenBaoLogin(_)), "{refused:?}");
}

#[tokio::test]
async fn the_service_serves_releases_until_it_is_told_to_stop() {
    let server = JetStreamTestServer::start().await;
    create_credential_stream(&server).await;
    let (openbao, _openbao) = fake_openbao().await;
    let shutdown = CancellationToken::new();
    let running = tokio::spawn(service(server.address(), &openbao).run(shutdown.clone()));
    let caller = server.client().await;
    let subject = format!(
        "{}.secrets.v1.service.billing-runner.SecretExchangeService.ResolveForRelease",
        KeyPair::new_account().public_key()
    );

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
}

#[tokio::test]
async fn a_service_told_to_stop_before_it_serves_stops_cleanly() {
    let server = JetStreamTestServer::start().await;
    create_credential_stream(&server).await;
    let (openbao, _openbao) = fake_openbao().await;
    let shutdown = CancellationToken::new();
    shutdown.cancel();

    tokio::time::timeout(STOPPED_WITHIN, service(server.address(), &openbao).run(shutdown))
        .await
        .expect("the service did not stop when told to")
        .unwrap();
}
