//! The release endpoint on a real server, deployed the way ADR#0082 Decision 1
//! deploys it: PLATFORM exports the endpoint with the caller's account as the
//! subject's first token, and each tenant account imports only its own token,
//! calling without it while the import inserts it. That inserted token and the
//! `service.{id}` the tenant called on are the only caller the endpoint sees.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use async_nats::client::SubscribeError;
use async_nats::{Client, HeaderMap, Message, RequestErrorKind, Subscriber};
use buffa::MessageField;
use bytes::Bytes;
use futures::StreamExt;
use grpc_nats_micro::ContentType;
use grpc_nats_micro::constants::HEADER_CONTENT_TYPE;
use grpc_nats_micro::status_codec::decode_reply;
use nkeys::{KeyPair, XKey};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use trogon_credential_aggregate::proto::active_state_to_proto;
use trogon_credential_domain::{
    AllowedHosts, AllowedRuntimeServices, ConfiguredReleasePolicy, CredentialFingerprint, CredentialIdentity,
    CredentialKind, CredentialOwnerId, CredentialRef, CredentialScope, InjectionLocation, InjectionLocations,
    RuntimeReleasePolicy, SourceKind,
};
use trogon_nats::test_support::CoreTestServer;
use trogon_secret_store::{InMemorySecretStore, SecretStorePut};
use trogon_secrets_service::{
    AdmissionLimit, CredentialRelease, InMemoryCredentialStates, RELEASE_SUBSCRIPTION, RecordingReleaseAudit,
    ReleaseRequest, ReleaseServer, ReleaseServerError, StaticReleasePolicies, StaticTenantOwners, TenantAccount,
    XkeySealer,
};
use trogon_std::SecretString;
use trogon_std::time::FixedEpochClock;
use trogonai_proto::connections::types::v1alpha1::AuditCorrelationId;
use trogonai_proto::gateway::credentials::state_v1::CredentialStateSnapshot;
use trogonai_proto::policy::egress_permissions::v1alpha1::EgressHost;
use trogonai_proto::secrets::exchange::v1alpha1::release_location::Value;
use trogonai_proto::secrets::exchange::v1alpha1::resolve_for_release_response::Result as Outcome;
use trogonai_proto::secrets::exchange::v1alpha1::{
    CredentialAddress, ReleaseLocation, RequesterXkey, ResolveFailureCategory, ResolveForReleaseRequest,
    ResolveForReleaseResponse,
};

const RUNTIME_SERVICE: &str = "billing-runner";
const OTHER_SERVICE: &str = "deploy-runner";
const HOST: &str = "api.github.com";
const PORT: u32 = 8443;
const HEADER: &str = "authorization";
const NOW: u64 = 1_700_000_000;
const READY_WITHIN: Duration = Duration::from_secs(5);
const TAPPED_WITHIN: Duration = Duration::from_secs(5);

/// The subject a runtime service calls on inside its own tenant account.
fn local_subject(service: &str) -> String {
    format!("secrets.v1.service.{service}.SecretExchangeService.ResolveForRelease")
}

/// The subject the endpoint sees for a call from `tenant`'s `service`, once the
/// import has inserted the tenant's token.
fn platform_subject(tenant: &TenantAccount, service: &str) -> String {
    format!("{tenant}.{}", local_subject(service))
}

/// PLATFORM exports the endpoint with the caller's account in the first token,
/// so the server refuses any import of a token other than the importer's own.
/// Each tenant account is named by its account key, which is the token the
/// server checks the import against.
fn accounts(password: &str, tenant_a: &TenantAccount, tenant_b: &TenantAccount) -> String {
    let tenant = |user: &str, account: &TenantAccount| {
        format!(
            r#"{account} {{
                users: [{{user: "{user}", password: "{password}"}}]
                imports: [{{service: {{account: PLATFORM, subject: "{account}.secrets.v1.>"}}, to: "secrets.v1.>"}}]
            }}"#
        )
    };
    format!(
        r#"accounts {{
            PLATFORM {{
                users: [{{user: "platform", password: "{password}"}}]
                exports: [{{service: "*.secrets.v1.>", account_token_position: 1}}]
            }}
            {}
            {}
        }}"#,
        tenant("tenant-a", tenant_a),
        tenant("tenant-b", tenant_b),
    )
}

/// One tenant account, its connection, and the credential its owner holds.
struct Tenant {
    account: TenantAccount,
    client: Client,
    credential: CredentialIdentity,
}

impl Tenant {
    async fn connect(server: &CoreTestServer, user: &str, password: &str, account: TenantAccount, owner: &str) -> Self {
        Self {
            account,
            client: connect(server, user, password).await,
            credential: credential(owner, CredentialKind::WebhookSecret),
        }
    }

    /// Calls `subject` from this tenant with `payload` as is, beside whatever
    /// `planted` headers the caller adds.
    async fn call(&self, subject: String, planted: &[(&str, String)], payload: Vec<u8>) -> Message {
        let mut headers = HeaderMap::new();
        headers.insert(HEADER_CONTENT_TYPE, ContentType::Protobuf.header_value());
        for (name, value) in planted {
            headers.insert(*name, value.as_str());
        }
        self.client
            .request_with_headers(subject, headers, Bytes::from(payload))
            .await
            .unwrap()
    }

    /// Asks, as `service` in this tenant, for `credential` sealed to `requester`.
    async fn ask(&self, service: &str, credential: &CredentialIdentity, requester: &XKey) -> Message {
        self.call(local_subject(service), &[], encode(&wire(credential, requester)))
            .await
    }
}

/// The endpoint served from PLATFORM over the release core, with tenant A
/// holding `project-a` and tenant B holding `project-b`, each owner holding one
/// credential its policy releases to `billing-runner` only.
struct Deployment {
    platform: Client,
    a: Tenant,
    b: Tenant,
    service_xkey: XKey,
    audit: RecordingReleaseAudit,
    states: InMemoryCredentialStates,
    shutdown: CancellationToken,
    serving: JoinHandle<Result<(), ReleaseServerError<SubscribeError>>>,
    _server: CoreTestServer,
}

impl Deployment {
    async fn start() -> Self {
        let password = uuid::Uuid::new_v4().to_string();
        let account_a = TenantAccount::parse(&KeyPair::new_account().public_key()).unwrap();
        let account_b = TenantAccount::parse(&KeyPair::new_account().public_key()).unwrap();
        let server = CoreTestServer::start_with_config(&accounts(&password, &account_a, &account_b)).await;
        let platform = connect(&server, "platform", &password).await;
        let a = Tenant::connect(&server, "tenant-a", &password, account_a, "project-a").await;
        let b = Tenant::connect(&server, "tenant-b", &password, account_b, "project-b").await;

        let store = InMemorySecretStore::default();
        let states = InMemoryCredentialStates::new();
        let mut owners = StaticTenantOwners::new();
        let mut policies = StaticReleasePolicies::new();
        for (tenant, value) in [(&a, "fake-github-secret-a"), (&b, "fake-github-secret-b")] {
            let stored = store
                .put(tenant.credential.clone(), SecretString::new(value).unwrap())
                .await
                .unwrap();
            states.set(tenant.credential.key(), active(&stored));
            owners = owners.with_owner(tenant.account.clone(), tenant.credential.owner_id().clone());
            policies = policies.with_policy(tenant.credential.key(), policy());
        }
        let service_xkey = XKey::new();
        let sealer = XkeySealer::from_seed(&SecretString::new(service_xkey.seed().unwrap()).unwrap()).unwrap();
        let audit = RecordingReleaseAudit::new();
        let release = CredentialRelease::new(
            owners,
            policies,
            states.clone(),
            store,
            audit.clone(),
            sealer,
            FixedEpochClock::from_secs(NOW),
        );
        let shutdown = CancellationToken::new();
        let serving = tokio::spawn(
            ReleaseServer::new(platform.clone(), Arc::new(release), AdmissionLimit::default()).run(shutdown.clone()),
        );

        let deployment = Self {
            platform,
            a,
            b,
            service_xkey,
            audit,
            states,
            shutdown,
            serving,
            _server: server,
        };
        deployment.wait_until_serving().await;
        deployment
    }

    /// Calls until the endpoint has joined its queue group. The probe is sent
    /// inside PLATFORM, because a tenant's call made before the endpoint
    /// subscribes meets the import's own interest and waits out its timeout
    /// instead of being told no one responds. It carries no payload, so the
    /// endpoint refuses it before the release core and audits nothing.
    async fn wait_until_serving(&self) {
        tokio::time::timeout(READY_WITHIN, async {
            loop {
                match self
                    .platform
                    .request(platform_subject(&self.a.account, RUNTIME_SERVICE), Bytes::new())
                    .await
                {
                    Ok(_) => return,
                    Err(error) if error.kind() == RequestErrorKind::NoResponders => {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    Err(error) => panic!("the readiness probe failed: {error}"),
                }
            }
        })
        .await
        .expect("the release endpoint did not join its queue group");
        assert!(self.audit.releases().is_empty() && self.audit.denials().is_empty());
    }

    /// A plain PLATFORM subscriber beside the queue group, so a test sees each
    /// call exactly as it reached the endpoint.
    async fn tap(&self) -> Subscriber {
        let tap = self.platform.subscribe(RELEASE_SUBSCRIPTION).await.unwrap();
        self.platform.flush().await.unwrap();
        tap
    }

    /// Opens a sealed access as `requester`, with this service as the sender.
    fn open(&self, outcome: Outcome, requester: &XKey) -> Vec<u8> {
        let Outcome::Access(access) = outcome else {
            panic!("a release answered {outcome:?}");
        };
        let sender = XKey::from_public_key(&self.service_xkey.public_key()).unwrap();
        requester.open(&access.xkey_sealed_value, &sender).unwrap()
    }

    /// The tenant and runtime service of every call the core denied, with
    /// the reason the audit recorded.
    fn denials(&self) -> Vec<(String, String, &'static str)> {
        self.audit
            .denials()
            .iter()
            .map(|record| {
                let (tenant, service) = caller(record.request());
                (tenant, service, record.reason().code())
            })
            .collect()
    }

    /// The tenant and runtime service of every call the core released to.
    fn releases(&self) -> Vec<(String, String)> {
        self.audit
            .releases()
            .iter()
            .map(|record| caller(record.request()))
            .collect()
    }

    async fn stop(self) {
        self.shutdown.cancel();
        self.serving.await.unwrap().unwrap();
    }
}

async fn connect(server: &CoreTestServer, user: &str, password: &str) -> Client {
    async_nats::ConnectOptions::new()
        .user_and_password(user.to_owned(), password.to_owned())
        .connect(server.address())
        .await
        .unwrap()
}

fn credential(owner: &str, kind: CredentialKind) -> CredentialIdentity {
    CredentialIdentity::derive(
        CredentialScope::source(CredentialOwnerId::new(owner).unwrap(), SourceKind::GitHub),
        kind,
    )
}

fn active(credential: &CredentialRef) -> CredentialStateSnapshot {
    CredentialStateSnapshot {
        state: Some(active_state_to_proto(credential, &CredentialFingerprint::new("fingerprint").unwrap(), &[]).into()),
    }
}

fn policy() -> ConfiguredReleasePolicy {
    ConfiguredReleasePolicy::try_from(
        RuntimeReleasePolicy::default()
            .with_allowed_hosts(AllowedHosts::only([HOST]).unwrap())
            .with_allowed_runtime_services(AllowedRuntimeServices::only([RUNTIME_SERVICE]).unwrap())
            .with_injection_locations(InjectionLocations::new([InjectionLocation::header(HEADER).unwrap()])),
    )
    .unwrap()
}

fn wire(credential: &CredentialIdentity, requester: &XKey) -> ResolveForReleaseRequest {
    ResolveForReleaseRequest {
        credential: MessageField::some(CredentialAddress {
            credential_id: credential.key().credential_id().to_string(),
            owner_id: credential.owner_id().to_string(),
        }),
        destination_host: MessageField::some(EgressHost {
            host: HOST.to_owned(),
            port: PORT,
        }),
        location: MessageField::some(ReleaseLocation {
            value: Some(Value::Header(HEADER.to_owned())),
        }),
        correlation_id: MessageField::some(AuditCorrelationId {
            value: "use-1".to_owned(),
        }),
        requester_xkey: MessageField::some(RequesterXkey {
            public_key: requester.public_key(),
        }),
    }
}

fn encode(wire: &ResolveForReleaseRequest) -> Vec<u8> {
    ContentType::Protobuf.encode(wire).unwrap()
}

fn outcome(reply: &Message) -> Outcome {
    decode_reply::<ResolveForReleaseResponse>(reply.headers.as_ref(), &reply.payload, ContentType::Protobuf)
        .unwrap()
        .result
        .unwrap()
}

fn assert_denied(reply: &Message) {
    let Outcome::Failure(failure) = outcome(reply) else {
        panic!("a refused call was released");
    };
    assert_eq!(
        failure.category.as_known(),
        Some(ResolveFailureCategory::RESOLVE_FAILURE_CATEGORY_DENIED)
    );
    assert!(failure.retry_after.is_unset());
}

fn caller(request: &ReleaseRequest) -> (String, String) {
    (request.tenant().to_string(), request.runtime_service().to_string())
}

/// Appends `value` to an encoded message as the length-delimited field
/// `number`, which the contract does not define.
fn plant_field(payload: &mut Vec<u8>, number: u64, value: &str) {
    varint(payload, number << 3 | 2);
    varint(payload, value.len() as u64);
    payload.extend_from_slice(value.as_bytes());
}

fn varint(payload: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        payload.push((value & 0x7f) as u8 | 0x80);
        value >>= 7;
    }
    payload.push(value as u8);
}

#[tokio::test]
async fn a_tenant_cannot_reach_the_endpoint_under_another_tenants_token() {
    let deployment = Deployment::start().await;
    let requester = XKey::new();

    let reached = deployment
        .a
        .client
        .request(
            platform_subject(&deployment.b.account, RUNTIME_SERVICE),
            Bytes::from(encode(&wire(&deployment.b.credential, &requester))),
        )
        .await;

    assert_eq!(reached.unwrap_err().kind(), RequestErrorKind::NoResponders);
    assert!(deployment.denials().is_empty());
    assert!(deployment.releases().is_empty());
    deployment.stop().await;
}

#[tokio::test]
async fn another_tenants_credential_is_denied_exactly_like_one_that_does_not_exist() {
    let deployment = Deployment::start().await;
    let requester = XKey::new();
    let a = &deployment.a;
    let missing = credential("project-a", CredentialKind::SigningSecret);

    let theirs = a.ask(RUNTIME_SERVICE, &deployment.b.credential, &requester).await;
    let nonexistent = a.ask(RUNTIME_SERVICE, &missing, &requester).await;

    assert_denied(&theirs);
    assert_eq!(theirs.payload, nonexistent.payload);
    assert_eq!(theirs.headers, nonexistent.headers);
    let tenant_a = a.account.to_string();
    assert_eq!(
        deployment.denials(),
        vec![
            (tenant_a.clone(), RUNTIME_SERVICE.to_owned(), "owner_not_held"),
            (tenant_a, RUNTIME_SERVICE.to_owned(), "no_release_policy"),
        ]
    );
    assert_eq!(deployment.states.reads(), 0);

    let own_requester = XKey::new();
    let own = deployment
        .b
        .ask(RUNTIME_SERVICE, &deployment.b.credential, &own_requester)
        .await;
    assert_eq!(deployment.open(outcome(&own), &own_requester), b"fake-github-secret-b");
    deployment.stop().await;
}

#[tokio::test]
async fn identity_planted_in_headers_or_body_is_ignored() {
    let deployment = Deployment::start().await;
    let a = &deployment.a;
    let tenant_b = deployment.b.account.to_string();
    let planted_headers = [
        ("Trogon-Tenant", tenant_b.clone()),
        ("Trogon-Runtime-Service", RUNTIME_SERVICE.to_owned()),
    ];
    let planted_body = |credential: &CredentialIdentity, requester: &XKey| {
        let mut payload = encode(&wire(credential, requester));
        plant_field(&mut payload, 100, &tenant_b);
        plant_field(&mut payload, 101, RUNTIME_SERVICE);
        payload
    };
    let mut tap = deployment.tap().await;
    let own_requester = XKey::new();
    let other_requester = XKey::new();

    let own = a
        .call(
            local_subject(RUNTIME_SERVICE),
            &planted_headers,
            planted_body(&a.credential, &own_requester),
        )
        .await;
    let theirs = a
        .call(
            local_subject(RUNTIME_SERVICE),
            &planted_headers,
            planted_body(&deployment.b.credential, &other_requester),
        )
        .await;
    let other_service = a
        .call(
            local_subject(OTHER_SERVICE),
            &planted_headers,
            planted_body(&a.credential, &other_requester),
        )
        .await;

    assert_eq!(deployment.open(outcome(&own), &own_requester), b"fake-github-secret-a");
    assert_denied(&theirs);
    assert_denied(&other_service);
    let tenant_a = a.account.to_string();
    assert_eq!(
        deployment.releases(),
        vec![(tenant_a.clone(), RUNTIME_SERVICE.to_owned())]
    );
    assert_eq!(
        deployment.denials(),
        vec![
            (tenant_a.clone(), RUNTIME_SERVICE.to_owned(), "owner_not_held"),
            (tenant_a, OTHER_SERVICE.to_owned(), "policy_refused_runtime_service"),
        ]
    );
    for (service, credential, requester) in [
        (RUNTIME_SERVICE, &a.credential, &own_requester),
        (RUNTIME_SERVICE, &deployment.b.credential, &other_requester),
        (OTHER_SERVICE, &a.credential, &other_requester),
    ] {
        let reached = tokio::time::timeout(TAPPED_WITHIN, tap.next()).await.unwrap().unwrap();
        assert_eq!(reached.subject.as_str(), platform_subject(&a.account, service));
        let headers = reached.headers.unwrap();
        for (name, value) in &planted_headers {
            assert_eq!(headers.get(*name).map(|value| value.as_str()), Some(value.as_str()));
        }
        assert_eq!(reached.payload.as_ref(), planted_body(credential, requester).as_slice());
    }
    deployment.stop().await;
}

#[tokio::test]
async fn a_runtime_service_opens_its_own_tenants_credential_with_its_xkey() {
    let deployment = Deployment::start().await;
    let requester = XKey::new();
    let a = &deployment.a;

    let reply = a.ask(RUNTIME_SERVICE, &a.credential, &requester).await;

    assert_eq!(deployment.open(outcome(&reply), &requester), b"fake-github-secret-a");
    let Outcome::Access(access) = outcome(&reply) else {
        panic!("a release answered a failure");
    };
    let sender = XKey::from_public_key(&deployment.service_xkey.public_key()).unwrap();
    assert!(XKey::new().open(&access.xkey_sealed_value, &sender).is_err());
    assert_eq!(
        deployment.releases(),
        vec![(a.account.to_string(), RUNTIME_SERVICE.to_owned())]
    );
    assert!(deployment.denials().is_empty());
    deployment.stop().await;
}
