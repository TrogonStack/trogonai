//! The release call against the real secrets service, on a real server: the
//! client asks on the subject the service subscribes to, with the payload it
//! decodes, and opens what the service seals. The fake is held to the same
//! subject and payload.

#![cfg(feature = "test-support")]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::num::NonZeroU16;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_nats::client::SubscribeError;
use async_nats::{Client, RequestErrorKind, Subscriber};
use buffa::Message;
use bytes::Bytes;
use futures::StreamExt;
use nkeys::error::ErrorKind;
use nkeys::{KeyPair, XKey};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use trogon_credential_aggregate::proto::active_state_to_proto;
use trogon_credential_domain::{
    AllowedHosts, AllowedRuntimeServices, AuditCorrelationId, ConfiguredReleasePolicy, CredentialFingerprint,
    CredentialIdentity, CredentialKind, CredentialOwnerId, CredentialRef, CredentialScope, InjectionLocation,
    InjectionLocations, ReleaseHost, RuntimeReleasePolicy, RuntimeServiceId, SourceKind,
};
use trogon_nats::test_support::CoreTestServer;
use trogon_secret_store::{InMemorySecretStore, SecretStorePut};
use trogon_secrets_client::{
    FakeOutcome, FakeSecretsService, ReleaseClient, ReleaseClientConfig, ReleaseDestination, ReleaseError,
    ReleaseRequest, RuntimeService, ServiceXkey, TenantAccount, UnsealError,
};
use trogon_secrets_service::{
    AdmissionLimit, CredentialRelease, InMemoryCredentialStates, RELEASE_SUBSCRIPTION, RELEASE_TTL_BOUND,
    RecordingReleaseAudit, ReleaseServer, ReleaseServerError, StaticReleasePolicies, StaticTenantOwners,
    TenantAccount as ServiceTenant, XkeySealer,
};
use trogon_std::SecretString;
use trogon_std::time::FixedEpochClock;
use trogonai_proto::gateway::credentials::state_v1::CredentialStateSnapshot;
use trogonai_proto::secrets::exchange::v1alpha1::ResolveForReleaseRequest;

const OWNER: &str = "project-a";
const RUNTIME_SERVICE: &str = "billing-runner";
const OTHER_SERVICE: &str = "deploy-runner";
const HOST: &str = "api.github.com";
const PORT: u16 = 8443;
const HEADER: &str = "authorization";
const VALUE: &str = "fake-github-secret";
const NOW: u64 = 1_700_000_000;
const READY_WITHIN: Duration = Duration::from_secs(5);
const TAPPED_WITHIN: Duration = Duration::from_secs(5);

/// The release core served on a real server, with one tenant whose owner holds
/// one credential its policy releases to `billing-runner` only.
struct Deployment {
    client: Client,
    platform: Client,
    tenant: TenantAccount,
    credential: CredentialIdentity,
    service_xkey: XKey,
    shutdown: CancellationToken,
    serving: JoinHandle<Result<(), ReleaseServerError<SubscribeError>>>,
    _server: CoreTestServer,
}

impl Deployment {
    async fn start() -> Self {
        let server = CoreTestServer::start().await;
        let platform = async_nats::connect(server.address()).await.unwrap();
        let client = async_nats::connect(server.address()).await.unwrap();
        let account = KeyPair::new_account().public_key();
        let credential = CredentialIdentity::derive(
            CredentialScope::source(CredentialOwnerId::new(OWNER).unwrap(), SourceKind::GitHub),
            CredentialKind::WebhookSecret,
        );

        let store = InMemorySecretStore::default();
        let stored = store
            .put(credential.clone(), SecretString::new(VALUE).unwrap())
            .await
            .unwrap();
        let states = InMemoryCredentialStates::new();
        states.set(credential.key(), active(&stored));
        let owners = StaticTenantOwners::new()
            .with_owner(ServiceTenant::parse(&account).unwrap(), credential.owner_id().clone());
        let policies = StaticReleasePolicies::new().with_policy(credential.key(), policy());
        let service_xkey = XKey::new();
        let sealer = XkeySealer::from_seed(&SecretString::new(service_xkey.seed().unwrap()).unwrap()).unwrap();
        let release = CredentialRelease::new(
            owners,
            policies,
            states,
            store,
            RecordingReleaseAudit::new(),
            sealer,
            FixedEpochClock::from_secs(NOW),
        );
        let shutdown = CancellationToken::new();
        let serving = tokio::spawn(
            ReleaseServer::new(platform.clone(), Arc::new(release), AdmissionLimit::default()).run(shutdown.clone()),
        );

        let deployment = Self {
            client,
            platform,
            tenant: TenantAccount::parse(&account).unwrap(),
            credential,
            service_xkey,
            shutdown,
            serving,
            _server: server,
        };
        deployment.wait_until_serving().await;
        deployment
    }

    /// Calls until the endpoint has joined its queue group. The probe carries
    /// no payload, so the endpoint refuses it before the release core.
    async fn wait_until_serving(&self) {
        let probe = format!(
            "{}.secrets.v1.service.{RUNTIME_SERVICE}.SecretExchangeService.ResolveForRelease",
            self.tenant
        );
        tokio::time::timeout(READY_WITHIN, async {
            loop {
                match self.platform.request(probe.clone(), Bytes::new()).await {
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
    }

    /// A plain subscriber beside the queue group, on the subscription the
    /// service serves, so a test sees each call as it reached the service.
    ///
    /// The call goes out on another connection, so the server must hold the
    /// tap before it routes the call. A flush only empties the socket. A
    /// request no one serves, sent after the subscribe on the same
    /// connection, comes back only once the server has read both.
    async fn tap(&self) -> Subscriber {
        let tap = self.platform.subscribe(RELEASE_SUBSCRIPTION).await.unwrap();
        let unserved = self.platform.request("release-contract.tap", Bytes::new()).await;
        assert!(
            matches!(&unserved, Err(error) if error.kind() == RequestErrorKind::NoResponders),
            "the server did not answer the tap's round trip: {unserved:?}"
        );
        tap
    }

    fn pinned(&self) -> ServiceXkey {
        ServiceXkey::from_public_key(&self.service_xkey.public_key()).unwrap()
    }

    fn release_client(&self, service: &str, pinned: ServiceXkey) -> ReleaseClient<Client, FixedEpochClock> {
        ReleaseClient::new(
            self.client.clone(),
            FixedEpochClock::from_secs(NOW),
            ReleaseClientConfig::new(runtime_service(service), pinned),
        )
    }

    fn request(&self, tenant: TenantAccount) -> ReleaseRequest {
        ReleaseRequest::new(
            tenant,
            self.credential.key(),
            ReleaseDestination::new(&ReleaseHost::new(HOST).unwrap(), NonZeroU16::new(PORT).unwrap()),
            InjectionLocation::header(HEADER).unwrap(),
            AuditCorrelationId::new("use-1").unwrap(),
        )
    }

    async fn stop(self) {
        self.shutdown.cancel();
        self.serving.await.unwrap().unwrap();
    }
}

fn runtime_service(service: &str) -> RuntimeService {
    RuntimeService::new(&RuntimeServiceId::new(service).unwrap()).unwrap()
}

fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(NOW)
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

async fn tapped(tap: &mut Subscriber) -> (String, ResolveForReleaseRequest) {
    let message = tokio::time::timeout(TAPPED_WITHIN, tap.next())
        .await
        .expect("the call reached the service")
        .unwrap();
    let request = ResolveForReleaseRequest::decode_from_slice(&message.payload).unwrap();
    (message.subject.to_string(), request)
}

#[tokio::test]
async fn is_denied_a_credential_its_tenant_does_not_own() {
    let deployment = Deployment::start().await;
    let stranger = TenantAccount::parse(&KeyPair::new_account().public_key()).unwrap();

    let refused = deployment
        .release_client(RUNTIME_SERVICE, deployment.pinned())
        .release(&deployment.request(stranger))
        .await
        .unwrap_err();

    assert!(matches!(refused, ReleaseError::Denied));
    deployment.stop().await;
}

#[tokio::test]
async fn is_denied_as_a_runtime_service_the_policy_does_not_name() {
    let deployment = Deployment::start().await;

    let refused = deployment
        .release_client(OTHER_SERVICE, deployment.pinned())
        .release(&deployment.request(deployment.tenant.clone()))
        .await
        .unwrap_err();

    assert!(matches!(refused, ReleaseError::Denied));
    deployment.stop().await;
}

#[tokio::test]
async fn does_not_open_a_release_when_pinned_to_another_service_key() {
    let deployment = Deployment::start().await;
    let impostor = ServiceXkey::from_public_key(&XKey::new().public_key()).unwrap();

    let refused = deployment
        .release_client(RUNTIME_SERVICE, impostor)
        .release(&deployment.request(deployment.tenant.clone()))
        .await
        .unwrap_err();

    assert!(
        matches!(refused, ReleaseError::Unseal(UnsealError::NotOpened(source)) if source.kind() == ErrorKind::InvalidPayload)
    );
    deployment.stop().await;
}

#[tokio::test]
async fn opens_what_the_service_releases() {
    let deployment = Deployment::start().await;

    let released = deployment
        .release_client(RUNTIME_SERVICE, deployment.pinned())
        .release(&deployment.request(deployment.tenant.clone()))
        .await
        .unwrap();

    assert_eq!(released.value().as_str(), VALUE);
    assert!(released.valid_until() > now());
    assert!(released.valid_until() <= now() + RELEASE_TTL_BOUND);
    assert!(released.hold_until() <= released.valid_until());
    deployment.stop().await;
}

#[tokio::test]
async fn asks_on_the_subject_the_service_serves_with_the_payload_it_decodes() {
    let deployment = Deployment::start().await;
    let mut tap = deployment.tap().await;

    deployment
        .release_client(RUNTIME_SERVICE, deployment.pinned())
        .release(&deployment.request(deployment.tenant.clone()))
        .await
        .unwrap();

    let (subject, request) = tapped(&mut tap).await;
    assert_eq!(
        subject,
        format!(
            "{}.secrets.v1.service.{RUNTIME_SERVICE}.SecretExchangeService.ResolveForRelease",
            deployment.tenant
        )
    );
    assert_eq!(request.credential.owner_id, OWNER);
    assert_eq!(
        request.credential.credential_id,
        deployment.credential.key().credential_id().to_string()
    );
    assert!(XKey::from_public_key(&request.requester_xkey.public_key).is_ok());
    deployment.stop().await;
}

#[tokio::test]
async fn the_fake_receives_what_the_service_receives() {
    let deployment = Deployment::start().await;
    let mut tap = deployment.tap().await;
    let fake = FakeSecretsService::new();
    fake.script(FakeOutcome::release(
        SecretString::new(VALUE).unwrap(),
        now() + Duration::from_secs(60),
        0,
    ));
    let against_fake = ReleaseClient::new(
        fake.clone(),
        FixedEpochClock::from_secs(NOW),
        ReleaseClientConfig::new(runtime_service(RUNTIME_SERVICE), fake.service_xkey()),
    );
    let asked = deployment.request(deployment.tenant.clone());

    deployment
        .release_client(RUNTIME_SERVICE, deployment.pinned())
        .release(&asked)
        .await
        .unwrap();
    against_fake.release(&asked).await.unwrap();

    let (service_subject, mut service_request) = tapped(&mut tap).await;
    let received = fake.received();
    let mut fake_request = received[0].request().clone();
    assert_eq!(received[0].subject(), service_subject);
    assert_ne!(
        fake_request.requester_xkey.public_key,
        service_request.requester_xkey.public_key
    );
    fake_request.requester_xkey = Default::default();
    service_request.requester_xkey = Default::default();
    assert_eq!(fake_request, service_request);
    deployment.stop().await;
}
