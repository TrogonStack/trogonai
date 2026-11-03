#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, TimeZone, Utc};
use trogon_decider_runtime::{
    AppendFailure, AppendStreamRequest, AppendStreamResponse, AuthorizationDeniedError, CommandAuthorizer,
    CommandError, CommandPrincipal, InMemoryStore, PrincipalKind, ReadFrom, ReadStreamRequest, ReadStreamResponse,
    StreamAppend, StreamRead, UnauthorizedError,
};
use trogon_peer::{
    CallerId, CollaborationRef, Enroll, EnrollmentIdentity, InstallationId, NativeSessionId, PeerId, TenantId,
    TrustedPeerOwner,
};
use trogon_peer_service::{EnrollServiceError, PeerClock, PeerService};
use trogonai_proto::convert::timestamp_from_datetime;
use trogonai_proto::peers::{PeerEventCase, v1alpha1 as proto};

#[derive(Clone, Default)]
struct ObservedStore {
    inner: InMemoryStore,
    reads: Arc<AtomicUsize>,
    appends: Arc<AtomicUsize>,
}

impl StreamRead<PeerId> for ObservedStore {
    type Error = <InMemoryStore as StreamRead<str>>::Error;

    async fn read_stream(&self, request: ReadStreamRequest<'_, PeerId>) -> Result<ReadStreamResponse, Self::Error> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner
            .read_stream(ReadStreamRequest {
                stream_id: request.stream_id.as_str(),
                from: request.from,
            })
            .await
    }
}

impl StreamAppend<PeerId> for ObservedStore {
    type Error = <InMemoryStore as StreamAppend<str>>::Error;

    async fn append_stream(
        &self,
        request: AppendStreamRequest<'_, PeerId>,
    ) -> Result<AppendStreamResponse, Self::Error> {
        self.appends.fetch_add(1, Ordering::SeqCst);
        self.inner
            .append_stream(AppendStreamRequest {
                stream_id: request.stream_id.as_str(),
                stream_write_precondition: request.stream_write_precondition,
                events: request.events,
            })
            .await
    }

    fn classify_append_failure(&self, error: &Self::Error) -> AppendFailure {
        <InMemoryStore as StreamAppend<str>>::classify_append_failure(&self.inner, error)
    }
}

#[derive(Clone)]
struct FixedClock(DateTime<Utc>);

impl PeerClock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

#[derive(Clone)]
struct EnrollmentPolicy {
    owner: TrustedPeerOwner,
    collaboration: CollaborationRef,
    observed_principals: Arc<Mutex<Vec<CommandPrincipal>>>,
}

impl CommandAuthorizer<Enroll> for EnrollmentPolicy {
    fn authorize(&self, principal: &CommandPrincipal, command: &Enroll) -> Result<(), AuthorizationDeniedError> {
        self.observed_principals.lock().unwrap().push(principal.clone());
        if command.identity().owner() != &self.owner
            || command.collaboration() != &self.collaboration
            || principal.kind() != PrincipalKind::Agent
            || principal.id().as_str() != self.owner.caller().as_str()
        {
            return Err(AuthorizationDeniedError::new(
                "enrollment is outside the configured grant",
            ));
        }
        Ok(())
    }
}

fn owner() -> TrustedPeerOwner {
    TrustedPeerOwner::new(
        TenantId::new("tenant_alpha").unwrap(),
        CallerId::new("caller_alpha").unwrap(),
    )
}

fn request(collaboration: &str) -> proto::EnrollRequest {
    proto::EnrollRequest {
        installation_id: Some("installation_alpha".to_owned()),
        native_session_id: Some("session_alpha".to_owned()),
        connector_id: Some("connector_alpha".to_owned()),
        collaboration_ref: Some(collaboration.to_owned()),
        display_name: Some("Review".to_owned()),
    }
}

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap()
}

fn policy() -> EnrollmentPolicy {
    EnrollmentPolicy {
        owner: owner(),
        collaboration: CollaborationRef::new("collaboration_alpha").unwrap(),
        observed_principals: Arc::new(Mutex::new(Vec::new())),
    }
}

#[tokio::test]
async fn authorized_enrollment_persists_core_events_with_trusted_owner_and_service_time() {
    let store = ObservedStore::default();
    let policy = policy();
    let observed_principals = policy.observed_principals.clone();
    let service = PeerService::new(store.clone(), policy, FixedClock(now()));
    let expected_identity = EnrollmentIdentity::new(
        owner(),
        InstallationId::new("installation_alpha").unwrap(),
        NativeSessionId::new("session_alpha").unwrap(),
    );

    let response = service
        .enroll(&owner(), request("collaboration_alpha"))
        .await
        .expect("enroll authorized peer");

    assert_eq!(response.peer_id.as_deref(), Some(expected_identity.peer_id().as_str()));
    assert_eq!(response.attachment.attachment.peer_id, response.peer_id);
    assert_eq!(response.attachment.attachment.generation, Some(1));
    assert_eq!(
        response.attachment.attachment.connector_id.as_deref(),
        Some("connector_alpha")
    );
    assert_eq!(
        response.attachment.observed_at.as_option(),
        Some(&timestamp_from_datetime(&now()))
    );
    assert_eq!(
        response.attachment.expires_at.as_option(),
        Some(&timestamp_from_datetime(&(now() + chrono::Duration::seconds(10))))
    );
    assert_eq!(store.appends.load(Ordering::SeqCst), 1);
    {
        let principals = observed_principals.lock().unwrap();
        assert_eq!(principals.len(), 1);
        assert_eq!(principals[0].id().as_str(), owner().caller().as_str());
    }

    let read = store
        .inner
        .read_stream(ReadStreamRequest {
            stream_id: expected_identity.peer_id().as_str(),
            from: ReadFrom::Beginning,
        })
        .await
        .expect("inspect persisted core enrollment");
    assert_eq!(read.events.len(), 2);
    let enrolled = read.events[0]
        .decode::<proto::PeerEvent>()
        .unwrap()
        .into_decoded()
        .unwrap();
    assert!(matches!(
        enrolled.event,
        Some(PeerEventCase::PeerEnrolled(ref fact))
            if fact.enrollment.tenant_id.as_deref() == Some("tenant_alpha")
                && fact.enrollment.caller_id.as_deref() == Some("caller_alpha")
                && fact.enrollment.native_session_id.as_deref() == Some("session_alpha")
    ));
    let attached = read.events[1]
        .decode::<proto::PeerEvent>()
        .unwrap()
        .into_decoded()
        .unwrap();
    assert!(matches!(attached.event, Some(PeerEventCase::PeerAttached(_))));
}

#[tokio::test]
async fn collaboration_denial_precedes_every_store_read_and_write() {
    let store = ObservedStore::default();
    let policy = policy();
    let observed_principals = policy.observed_principals.clone();
    let service = PeerService::new(store.clone(), policy, FixedClock(now()));

    let result = service.enroll(&owner(), request("collaboration_denied")).await;

    assert!(matches!(
        result,
        Err(EnrollServiceError::Execution(CommandError::Unauthorized(
            UnauthorizedError::Denied(_)
        )))
    ));
    assert_eq!(observed_principals.lock().unwrap().len(), 1);
    assert_eq!(store.reads.load(Ordering::SeqCst), 0);
    assert_eq!(store.appends.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn repeated_prompt_resumes_same_live_connector_without_replacing_attachment() {
    let store = ObservedStore::default();
    let service = PeerService::new(store.clone(), policy(), FixedClock(now()));
    let first = service.enroll(&owner(), request("collaboration_alpha")).await.unwrap();
    let resumed = service.enroll(&owner(), request("collaboration_alpha")).await;
    assert!(resumed.is_ok(), "same live connector must resume: {resumed:?}");
    assert_eq!(first, resumed.unwrap());
    assert_eq!(store.appends.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn expired_connector_replacement_fences_generation_and_preserves_peer_identity() {
    let store = ObservedStore::default();
    let first = PeerService::new(store.clone(), policy(), FixedClock(now()))
        .enroll(&owner(), request("collaboration_alpha"))
        .await
        .unwrap();
    let mut replacement = request("collaboration_alpha");
    replacement.connector_id = Some("replacement".into());
    let resumed = PeerService::new(
        store.clone(),
        policy(),
        FixedClock(now() + chrono::Duration::seconds(10)),
    )
    .enroll(&owner(), replacement)
    .await;
    assert!(resumed.is_ok(), "expired connector must allow replacement: {resumed:?}");
    let resumed = resumed.unwrap();
    assert_eq!(resumed.peer_id, first.peer_id);
    assert_eq!(resumed.attachment.attachment.generation, Some(2));
    assert_eq!(
        resumed.attachment.attachment.connector_id.as_deref(),
        Some("replacement")
    );
}
