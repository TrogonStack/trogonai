use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use buffa::MessageField;
use nkeys::XKey;
use trogon_credential_aggregate::proto::{
    active_state_to_proto, revoked_to_proto_state, rotation_pending_to_proto_state,
};
use trogon_credential_domain::{
    AllowedHosts, AllowedRuntimeServices, AuditCorrelationId, ConfiguredReleasePolicy, CredentialFingerprint,
    CredentialIdentity, CredentialKind, CredentialOwnerId, CredentialRef, CredentialScope, CredentialStatus,
    CredentialVersion, InjectionLocation, InjectionLocations, RuntimeReleasePolicy, SourceKind,
};
use trogon_secret_store::{InMemorySecretStore, SecretStorePut, SecretStoreRevoke, SecretVerifier};
use trogon_std::SecretString;
use trogon_std::log_capture::{CapturedEvent, CapturedEvents, LevelFilter};
use trogon_std::time::FixedEpochClock;
use trogonai_proto::gateway::credentials::CredentialStateSnapshotCase;
use trogonai_proto::gateway::credentials::state_v1::{self, CredentialStateSnapshot};

use super::*;
use crate::fixtures::{CORRELATION_ID, HEADER, HOST, OWNER, RUNTIME_SERVICE, identity, request, tenant};
use crate::{
    CredentialStateReadError, InMemoryCredentialStates, RecordingReleaseAudit, SealError, SealedValue,
    StaticReleasePolicies, StaticTenantOwners, TenantAccount, XkeySealer,
};

const NOW: u64 = 1_700_000_000;
const VALUE: &str = "ghp_released_value";

#[derive(Clone)]
enum TestStore {
    Real(InMemorySecretStore),
    Failing(SecretStoreError),
    VerifierOnly,
}

impl SecretStoreCorrelatedGet for TestStore {
    type Error = SecretStoreError;

    async fn get_correlated(
        &self,
        credential: &CredentialRef,
        correlation_id: &AuditCorrelationId,
    ) -> Result<SecretMaterial, SecretStoreError> {
        match self {
            Self::Real(store) => store.get_correlated(credential, correlation_id).await,
            Self::Failing(error) => Err(error.clone()),
            Self::VerifierOnly => Ok(SecretMaterial::Verifier(SecretVerifier::new("verifier-hash").unwrap())),
        }
    }
}

#[derive(Clone)]
struct CountingSealer {
    inner: Option<XkeySealer>,
    calls: Arc<AtomicUsize>,
}

impl Sealer for CountingSealer {
    fn seal(&self, value: &SecretString, recipient: &RequesterXkey) -> Result<SealedValue, SealError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match &self.inner {
            Some(sealer) => sealer.seal(value, recipient),
            None => Err(SealError::new("sealer refused")),
        }
    }
}

struct Harness {
    tenant: TenantAccount,
    owners: StaticTenantOwners,
    policies: StaticReleasePolicies,
    states: InMemoryCredentialStates,
    memory: InMemorySecretStore,
    store: TestStore,
    audit: RecordingReleaseAudit,
    sealer: CountingSealer,
    service: XKey,
    requester: XKey,
    requester_xkey: String,
    events: CapturedEvents,
    _capture: tracing::subscriber::DefaultGuard,
}

impl Harness {
    async fn new() -> Self {
        let events = CapturedEvents::new();
        let capture = events.install(LevelFilter::TRACE);
        let tenant = tenant();
        let memory = InMemorySecretStore::default();
        let stored = memory.put(identity(), SecretString::new(VALUE).unwrap()).await.unwrap();
        let states = InMemoryCredentialStates::new();
        states.set(identity().key(), active(&stored));
        let service = XKey::new();
        let requester = XKey::new();
        Self {
            owners: StaticTenantOwners::new().with_owner(tenant.clone(), CredentialOwnerId::new(OWNER).unwrap()),
            policies: StaticReleasePolicies::new()
                .with_policy(identity().key(), policy(RuntimeReleasePolicy::default())),
            states,
            store: TestStore::Real(memory.clone()),
            memory,
            audit: RecordingReleaseAudit::new(),
            sealer: CountingSealer {
                inner: Some(XkeySealer::from_seed(&SecretString::new(service.seed().unwrap()).unwrap()).unwrap()),
                calls: Arc::new(AtomicUsize::new(0)),
            },
            requester_xkey: requester.public_key(),
            service,
            requester,
            tenant,
            events,
            _capture: capture,
        }
    }

    async fn release(&self) -> Result<SealedRelease, CredentialReleaseError> {
        CredentialRelease::new(
            self.owners.clone(),
            self.policies.clone(),
            self.states.clone(),
            self.store.clone(),
            self.audit.clone(),
            self.sealer.clone(),
            FixedEpochClock::from_secs(NOW),
        )
        .release(&request(&self.tenant), &self.requester_xkey)
        .await
    }

    fn set_state(&self, state: impl Into<CredentialStateSnapshotCase>) {
        self.states.set(
            identity().key(),
            CredentialStateSnapshot {
                state: Some(state.into()),
            },
        );
    }

    fn seals(&self) -> usize {
        self.sealer.calls.load(Ordering::SeqCst)
    }

    fn denial_reasons(&self) -> Vec<DenialReason> {
        self.audit
            .denials()
            .iter()
            .map(|record| record.reason().clone())
            .collect()
    }

    fn event(&self, message: &str) -> CapturedEvent {
        self.events
            .events()
            .into_iter()
            .find(|event| event.message() == Some(message))
            .unwrap()
    }

    fn assert_denied(&self, result: Result<SealedRelease, CredentialReleaseError>, reason: DenialReason) {
        assert_eq!(result, Err(CredentialReleaseError::Denied));
        assert_eq!(self.denial_reasons(), vec![reason.clone()]);
        assert!(self.audit.releases().is_empty());
        assert_eq!(self.seals(), 0);
        let event = self.event("credential release denied");
        assert_eq!(event.field("reason"), Some(reason.code()));
        assert_eq!(event.field("correlation_id"), Some(CORRELATION_ID));
    }

    fn assert_faulted(
        &self,
        result: Result<SealedRelease, CredentialReleaseError>,
        failure: CredentialReleaseError,
        code: &str,
    ) {
        assert_eq!(result, Err(failure));
        assert!(self.audit.denials().is_empty());
        let event = self.event("credential release failed");
        assert_eq!(event.field("fault"), Some(code));
        assert_eq!(event.field("correlation_id"), Some(CORRELATION_ID));
        assert!(event.field("error").is_some());
    }
}

fn policy(base: RuntimeReleasePolicy) -> ConfiguredReleasePolicy {
    policy_for(base, RUNTIME_SERVICE, HOST, HEADER)
}

fn policy_for(base: RuntimeReleasePolicy, service: &str, host: &str, header: &str) -> ConfiguredReleasePolicy {
    ConfiguredReleasePolicy::try_from(
        base.with_allowed_hosts(AllowedHosts::only([host]).unwrap())
            .with_allowed_runtime_services(AllowedRuntimeServices::only([service]).unwrap())
            .with_injection_locations(InjectionLocations::new([InjectionLocation::header(header).unwrap()])),
    )
    .unwrap()
}

fn active(credential: &CredentialRef) -> CredentialStateSnapshot {
    CredentialStateSnapshot {
        state: Some(active_proto(credential).into()),
    }
}

fn active_proto(credential: &CredentialRef) -> state_v1::ActiveCredentialState {
    active_state_to_proto(credential, &CredentialFingerprint::new("fingerprint").unwrap(), &[])
}

fn version_one() -> CredentialRef {
    CredentialRef::new(identity(), CredentialVersion::new(1).unwrap())
}

#[tokio::test]
async fn an_undecodable_requester_xkey_is_key_denied_before_any_lookup() {
    let mut harness = Harness::new().await;
    harness.requester_xkey = "not-a-key".to_owned();
    harness.owners = StaticTenantOwners::new();

    let result = harness.release().await;

    assert_eq!(result, Err(CredentialReleaseError::KeyDenied));
    assert_eq!(harness.states.reads(), 0);
    assert_eq!(harness.denial_reasons(), vec![DenialReason::RequesterXkeyInvalid]);
    assert_eq!(harness.seals(), 0);
}

#[tokio::test]
async fn the_store_reads_under_the_release_correlation_id() {
    let harness = Harness::new().await;

    harness.release().await.unwrap();

    assert_eq!(
        harness.memory.correlation_ids().await,
        vec![AuditCorrelationId::new(CORRELATION_ID).unwrap()]
    );
}

#[tokio::test]
async fn an_unknown_tenant_is_denied_without_reading_the_credential() {
    let mut harness = Harness::new().await;
    harness.owners = StaticTenantOwners::new();

    let result = harness.release().await;

    harness.assert_denied(result, DenialReason::UnknownTenant);
    assert_eq!(harness.states.reads(), 0);
}

#[tokio::test]
async fn an_owner_the_tenant_does_not_hold_is_denied() {
    let mut harness = Harness::new().await;
    harness.owners =
        StaticTenantOwners::new().with_owner(harness.tenant.clone(), CredentialOwnerId::new("project-2").unwrap());

    let result = harness.release().await;

    harness.assert_denied(result, DenialReason::OwnerNotHeld);
    assert_eq!(harness.states.reads(), 0);
}

#[tokio::test]
async fn a_credential_without_a_policy_is_denied() {
    let mut harness = Harness::new().await;
    harness.policies = StaticReleasePolicies::new();

    let result = harness.release().await;

    harness.assert_denied(result, DenialReason::NoReleasePolicy);
    assert_eq!(harness.states.reads(), 0);
}

#[tokio::test]
async fn a_policy_that_refuses_the_runtime_service_is_denied() {
    let mut harness = Harness::new().await;
    let refusing = policy_for(RuntimeReleasePolicy::default(), "other-runner", HOST, HEADER);
    harness.policies = StaticReleasePolicies::new().with_policy(identity().key(), refusing);

    let result = harness.release().await;

    harness.assert_denied(
        result,
        DenialReason::PolicyRefused(trogon_credential_domain::RuntimeReleaseDeniedError::RuntimeService {
            runtime_service: Some(trogon_credential_domain::RuntimeServiceId::new(RUNTIME_SERVICE).unwrap()),
        }),
    );
}

#[tokio::test]
async fn a_policy_that_refuses_the_host_is_denied() {
    let mut harness = Harness::new().await;
    let refusing = policy_for(
        RuntimeReleasePolicy::default(),
        RUNTIME_SERVICE,
        "api.stripe.com",
        HEADER,
    );
    harness.policies = StaticReleasePolicies::new().with_policy(identity().key(), refusing);

    let result = harness.release().await;

    harness.assert_denied(
        result,
        DenialReason::PolicyRefused(trogon_credential_domain::RuntimeReleaseDeniedError::Host {
            host: Some(HOST.to_owned()),
        }),
    );
}

#[tokio::test]
async fn a_policy_that_refuses_the_injection_location_is_denied() {
    let mut harness = Harness::new().await;
    let refusing = policy_for(RuntimeReleasePolicy::default(), RUNTIME_SERVICE, HOST, "x-api-key");
    harness.policies = StaticReleasePolicies::new().with_policy(identity().key(), refusing);

    let result = harness.release().await;

    harness.assert_denied(
        result,
        DenialReason::PolicyRefused(trogon_credential_domain::RuntimeReleaseDeniedError::InjectionLocation {
            location: InjectionLocation::header(HEADER).unwrap(),
        }),
    );
}

#[tokio::test]
async fn a_credential_with_no_events_is_denied() {
    let mut harness = Harness::new().await;
    harness.states = InMemoryCredentialStates::new();

    let result = harness.release().await;

    harness.assert_denied(result, DenialReason::UnknownCredential);
}

#[tokio::test]
async fn a_credential_that_releases_no_version_is_denied() {
    let harness = Harness::new().await;
    harness.set_state(revoked_to_proto_state(&version_one()));

    let result = harness.release().await;

    harness.assert_denied(result, DenialReason::NotReleasable);
}

#[tokio::test]
async fn a_version_the_store_no_longer_reads_is_denied() {
    let harness = Harness::new().await;
    harness.memory.revoke(&version_one()).await.unwrap();

    let result = harness.release().await;

    harness.assert_denied(result, DenialReason::VersionUnreadable);
}

#[tokio::test]
async fn a_credential_the_store_holds_only_a_verifier_for_is_denied() {
    let mut harness = Harness::new().await;
    harness.store = TestStore::VerifierOnly;

    let result = harness.release().await;

    harness.assert_denied(result, DenialReason::VerifierOnly);
}

#[tokio::test]
async fn a_denial_stands_when_its_audit_write_fails() {
    let mut harness = Harness::new().await;
    harness.policies = StaticReleasePolicies::new();
    harness.audit.refuse_denials();

    let result = harness.release().await;

    assert_eq!(result, Err(CredentialReleaseError::Denied));
    assert_eq!(harness.seals(), 0);
    let event = harness.event("credential release denial was not audited; the denial stands");
    assert_eq!(
        event.field("error"),
        Some("audit record was not acknowledged: denial record refused")
    );
    assert_eq!(event.field("correlation_id"), Some(CORRELATION_ID));
}

#[tokio::test]
async fn an_unreachable_credential_stream_is_unavailable() {
    let harness = Harness::new().await;
    harness.states.fail_with(CredentialStateReadError::Unavailable {
        message: "no responders".to_owned(),
    });

    let result = harness.release().await;

    harness.assert_faulted(result, CredentialReleaseError::Unavailable, "state_unavailable");
}

#[tokio::test]
async fn a_credential_stream_that_does_not_fold_is_an_integrity_failure() {
    let harness = Harness::new().await;
    harness.states.fail_with(CredentialStateReadError::Undecodable {
        message: "unknown event".to_owned(),
    });

    let result = harness.release().await;

    harness.assert_faulted(result, CredentialReleaseError::IntegrityFailure, "stream_undecodable");
}

#[tokio::test]
async fn a_releasable_state_that_does_not_decode_is_an_integrity_failure() {
    let harness = Harness::new().await;
    harness.set_state(state_v1::RotationPendingCredentialState {
        active: MessageField::none(),
    });

    let result = harness.release().await;

    harness.assert_faulted(result, CredentialReleaseError::IntegrityFailure, "state_undecodable");
}

#[tokio::test]
async fn a_state_that_names_another_credential_is_an_integrity_failure() {
    let harness = Harness::new().await;
    let other = CredentialIdentity::derive(
        CredentialScope::source(CredentialOwnerId::new(OWNER).unwrap(), SourceKind::GitHub),
        CredentialKind::AppToken,
    );
    harness.set_state(active_proto(&CredentialRef::new(
        other,
        CredentialVersion::new(1).unwrap(),
    )));

    let result = harness.release().await;

    harness.assert_faulted(result, CredentialReleaseError::IntegrityFailure, "other_credential");
}

#[tokio::test]
async fn a_version_the_store_never_had_is_an_integrity_failure() {
    let mut harness = Harness::new().await;
    harness.store = TestStore::Real(InMemorySecretStore::default());

    let result = harness.release().await;

    harness.assert_faulted(result, CredentialReleaseError::IntegrityFailure, "store_missing");
}

#[tokio::test]
async fn an_unreachable_or_refusing_store_is_unavailable() {
    let errors = [
        SecretStoreError::BackendUnavailable {
            message: "connection refused".to_owned(),
        },
        SecretStoreError::Unwritable {
            credential: version_one(),
            status: CredentialStatus::Revoked,
        },
        SecretStoreError::InvalidGenerated {
            source: CredentialFingerprint::new("").unwrap_err().into(),
        },
    ];

    for error in errors {
        let mut harness = Harness::new().await;
        harness.store = TestStore::Failing(error);

        let result = harness.release().await;

        harness.assert_faulted(result, CredentialReleaseError::Unavailable, "store_unavailable");
    }
}

#[tokio::test]
async fn an_unacknowledged_release_record_is_unavailable_and_never_sealed() {
    let harness = Harness::new().await;
    harness.audit.refuse_releases();

    let result = harness.release().await;

    harness.assert_faulted(result, CredentialReleaseError::Unavailable, "audit_unavailable");
    assert!(harness.audit.releases().is_empty());
    assert_eq!(harness.seals(), 0);
}

#[tokio::test]
async fn a_value_that_cannot_be_sealed_is_internal_after_its_release_is_audited() {
    let mut harness = Harness::new().await;
    harness.sealer.inner = None;

    let result = harness.release().await;

    harness.assert_faulted(result, CredentialReleaseError::Internal, "seal_failed");
    assert_eq!(harness.audit.releases().len(), 1);
    assert_eq!(harness.seals(), 1);
}

#[tokio::test]
async fn releases_the_value_sealed_to_the_requester_from_the_service() {
    let harness = Harness::new().await;

    let release = harness.release().await.unwrap();

    let sender = XKey::from_public_key(&harness.service.public_key()).unwrap();
    let opened = harness
        .requester
        .open(release.sealed_value().as_bytes(), &sender)
        .unwrap();
    assert!(opened == VALUE.as_bytes());
    assert_eq!(release.bundle_generation(), 0);
    let valid_until = FixedEpochClock::from_secs(NOW).0 + Duration::from_secs(300);
    assert_eq!(release.valid_until(), valid_until);
    let record = ReleaseRecord::new(
        request(&harness.tenant),
        CredentialVersion::new(1).unwrap(),
        RequesterXkey::parse(&harness.requester_xkey).unwrap(),
        valid_until,
    );
    assert_eq!(harness.audit.releases(), vec![record]);
    assert!(harness.audit.denials().is_empty());
    let event = harness.event("credential released");
    assert_eq!(event.field("version"), Some("1"));
    assert_eq!(event.field("correlation_id"), Some(CORRELATION_ID));
    assert!(
        harness
            .events
            .events()
            .iter()
            .all(|event| !format!("{event:?}").contains(VALUE))
    );
}

#[tokio::test]
async fn a_pending_rotation_releases_the_version_it_would_replace() {
    let harness = Harness::new().await;
    harness.set_state(rotation_pending_to_proto_state(active_proto(&version_one())));

    let release = harness.release().await.unwrap();

    let sender = XKey::from_public_key(&harness.service.public_key()).unwrap();
    assert!(
        harness
            .requester
            .open(release.sealed_value().as_bytes(), &sender)
            .unwrap()
            == VALUE.as_bytes()
    );
}

#[tokio::test]
async fn a_policy_may_shorten_how_long_the_value_is_held() {
    let mut harness = Harness::new().await;
    let shorter = policy(
        RuntimeReleasePolicy::default()
            .with_cache_ttl_override(Duration::from_secs(60))
            .unwrap(),
    );
    harness.policies = StaticReleasePolicies::new().with_policy(identity().key(), shorter);

    let release = harness.release().await.unwrap();

    assert_eq!(
        release.valid_until(),
        FixedEpochClock::from_secs(NOW).0 + Duration::from_secs(60)
    );
}

#[tokio::test]
async fn a_policy_never_lengthens_how_long_the_value_is_held() {
    let mut harness = Harness::new().await;
    let longer = policy(
        RuntimeReleasePolicy::default()
            .with_cache_ttl_override(Duration::from_secs(3_600))
            .unwrap(),
    );
    harness.policies = StaticReleasePolicies::new().with_policy(identity().key(), longer);

    let release = harness.release().await.unwrap();

    assert_eq!(
        release.valid_until(),
        FixedEpochClock::from_secs(NOW).0 + RELEASE_TTL_BOUND
    );
}
