#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::{future::Future, pin::Pin};
type ReadInterleave = (usize, Pin<Box<dyn Future<Output = ()> + Send>>);
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
use trogon_peer_service::{EnrollServiceError, PeerApplicationError, PeerClock, PeerOperation, PeerService};
use trogonai_proto::convert::timestamp_from_datetime;
use trogonai_proto::peers::{PeerEventCase, v1alpha1 as proto};

#[derive(Clone, Default)]
struct ObservedStore {
    inner: InMemoryStore,
    reads: Arc<AtomicUsize>,
    appends: Arc<AtomicUsize>,
    before_read: Arc<Mutex<Option<ReadInterleave>>>,
}

impl StreamRead<PeerId> for ObservedStore {
    type Error = <InMemoryStore as StreamRead<str>>::Error;

    async fn read_stream(&self, request: ReadStreamRequest<'_, PeerId>) -> Result<ReadStreamResponse, Self::Error> {
        let ordinal = self.reads.fetch_add(1, Ordering::SeqCst) + 1;
        let action = {
            let mut pending = self.before_read.lock().unwrap();
            if pending.as_ref().is_some_and(|(at, _)| *at == ordinal) {
                pending.take().map(|(_, action)| action)
            } else {
                None
            }
        };
        if let Some(action) = action {
            action.await;
        }
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
    allowed_peer: Option<PeerId>,
    allowed_operation: Option<PeerOperation>,
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
        allowed_peer: None,
        allowed_operation: None,
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
    assert_eq!(response.attachment.attachment_ref.peer_id, response.peer_id);
    assert_eq!(response.attachment.attachment_ref.generation, Some(1));
    assert_eq!(
        response.attachment.attachment_ref.connector_id.as_deref(),
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
async fn malformed_enrollment_reports_the_field_before_authorization_or_storage() {
    use trogon_peer::PeerValueError;
    use trogon_peer_service::EnrollmentField;
    let valid = request("collaboration_alpha");
    let cases = [
        (
            EnrollmentField::InstallationId,
            "installation_id",
            PeerValueError::Empty,
            proto::EnrollRequest {
                installation_id: Some(String::new()),
                ..valid.clone()
            },
        ),
        (
            EnrollmentField::NativeSessionId,
            "native_session_id",
            PeerValueError::InvalidCharacter,
            proto::EnrollRequest {
                native_session_id: Some("session\ninvalid".into()),
                ..valid.clone()
            },
        ),
        (
            EnrollmentField::ConnectorId,
            "connector_id",
            PeerValueError::InvalidCharacter,
            proto::EnrollRequest {
                connector_id: Some("connector.invalid".into()),
                ..valid.clone()
            },
        ),
        (
            EnrollmentField::CollaborationRef,
            "collaboration_ref",
            PeerValueError::Empty,
            proto::EnrollRequest {
                collaboration_ref: Some(String::new()),
                ..valid.clone()
            },
        ),
        (
            EnrollmentField::DisplayName,
            "display_name",
            PeerValueError::TooLong { maximum: 80 },
            proto::EnrollRequest {
                display_name: Some("é".repeat(41)),
                ..valid
            },
        ),
    ];
    for (expected_field, diagnostic_field, expected_source, invalid) in cases {
        let store = ObservedStore::default();
        let policy = policy();
        let observed = policy.observed_principals.clone();
        let service = PeerService::new(store.clone(), policy, FixedClock(now()));
        let error = service
            .enroll(&owner(), invalid)
            .await
            .expect_err("malformed enrollment must fail");
        assert!(
            error.to_string().contains(diagnostic_field),
            "the caller must be able to correct the rejected field"
        );
        assert!(
            matches!(error, EnrollServiceError::InvalidInput { field, source } if field == expected_field && source == expected_source)
        );
        assert!(observed.lock().unwrap().is_empty());
        assert_eq!(store.reads.load(Ordering::SeqCst), 0);
        assert_eq!(store.appends.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn absent_enrollment_fields_report_missing_input_without_authorizing_a_partial_identity() {
    use trogon_peer_service::EnrollmentField;
    let valid = request("collaboration_alpha");
    let cases = [
        (
            EnrollmentField::InstallationId,
            proto::EnrollRequest {
                installation_id: None,
                ..valid.clone()
            },
        ),
        (
            EnrollmentField::NativeSessionId,
            proto::EnrollRequest {
                native_session_id: None,
                ..valid.clone()
            },
        ),
        (
            EnrollmentField::ConnectorId,
            proto::EnrollRequest {
                connector_id: None,
                ..valid.clone()
            },
        ),
        (
            EnrollmentField::CollaborationRef,
            proto::EnrollRequest {
                collaboration_ref: None,
                ..valid.clone()
            },
        ),
        (
            EnrollmentField::DisplayName,
            proto::EnrollRequest {
                display_name: None,
                ..valid
            },
        ),
    ];
    for (expected, incomplete) in cases {
        let store = ObservedStore::default();
        let policy = policy();
        let observed = policy.observed_principals.clone();
        let service = PeerService::new(store.clone(), policy, FixedClock(now()));
        assert!(
            matches!(service.enroll(&owner(), incomplete).await, Err(EnrollServiceError::MissingInput { field }) if field == expected)
        );
        assert!(observed.lock().unwrap().is_empty());
        assert_eq!(store.reads.load(Ordering::SeqCst), 0);
        assert_eq!(store.appends.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn repeated_prompt_resumes_same_live_connector_without_replacing_attachment() {
    let store = ObservedStore::default();
    let service = PeerService::new(store.clone(), policy(), FixedClock(now()));
    let first = service.enroll(&owner(), request("collaboration_alpha")).await.unwrap();
    let peer = PeerId::parse(first.peer_id.clone().unwrap()).unwrap();
    let original = store
        .read_stream(ReadStreamRequest {
            stream_id: &peer,
            from: ReadFrom::Beginning,
        })
        .await
        .unwrap();
    let resumed = service.enroll(&owner(), request("collaboration_alpha")).await;
    assert!(resumed.is_ok(), "same live connector must resume: {resumed:?}");
    assert_eq!(first, resumed.unwrap());
    assert_eq!(
        store.appends.load(Ordering::SeqCst),
        2,
        "the create-only attempt must conflict before authorized resume"
    );
    assert_same_durable_history(
        &(store
            .read_stream(ReadStreamRequest {
                stream_id: &peer,
                from: ReadFrom::Beginning,
            })
            .await
            .unwrap()),
        &original,
    );
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
    assert_eq!(resumed.attachment.attachment_ref.generation, Some(2));
    assert_eq!(
        resumed.attachment.attachment_ref.connector_id.as_deref(),
        Some("replacement")
    );
}

impl CommandAuthorizer<trogon_peer::Attach> for EnrollmentPolicy {
    fn authorize(
        &self,
        principal: &CommandPrincipal,
        command: &trogon_peer::Attach,
    ) -> Result<(), AuthorizationDeniedError> {
        if command.identity().owner() != &self.owner
            || command.collaboration() != &self.collaboration
            || principal.kind() != PrincipalKind::Agent
            || principal.id().as_str() != self.owner.caller().as_str()
        {
            return Err(AuthorizationDeniedError::new("resume outside configured grant"));
        }
        Ok(())
    }
}

struct ServiceCoverageHeartbeatGrant {
    owner: TrustedPeerOwner,
    reference: trogon_peer::AttachmentRef,
}

impl CommandAuthorizer<trogon_peer::Heartbeat> for ServiceCoverageHeartbeatGrant {
    fn authorize(
        &self,
        principal: &CommandPrincipal,
        command: &trogon_peer::Heartbeat,
    ) -> Result<(), AuthorizationDeniedError> {
        if command.owner() != &self.owner
            || command.attachment() != &self.reference
            || principal.kind() != PrincipalKind::Agent
            || principal.id().as_str() != self.owner.caller().as_str()
        {
            return Err(AuthorizationDeniedError::new("heartbeat outside exact enrolled grant"));
        }
        Ok(())
    }
}

#[tokio::test]
async fn live_connector_resume_refuses_valid_history_beyond_the_4096_event_ceiling() {
    let store = ObservedStore::default();
    let service = PeerService::new(store.clone(), policy(), FixedClock(now()));
    let enrolled = service.enroll(&owner(), request("collaboration_alpha")).await.unwrap();
    let wire_reference = enrolled.attachment.attachment_ref.as_option().unwrap();
    let peer = PeerId::parse(wire_reference.peer_id.clone().unwrap()).unwrap();
    let reference = trogon_peer::AttachmentRef::new(
        peer.clone(),
        trogon_peer::ConnectorId::new(wire_reference.connector_id.clone().unwrap()).unwrap(),
        wire_reference.generation.unwrap(),
    )
    .unwrap();
    let grant = ServiceCoverageHeartbeatGrant {
        owner: owner(),
        reference: reference.clone(),
    };
    let heartbeat = trogon_peer::Heartbeat::new(
        owner(),
        reference,
        trogon_peer::PeerObservation::new(proto::PeerAvailability::PEER_AVAILABILITY_READY, None, None).unwrap(),
        now(),
    );
    trogon_decider_runtime::CommandExecution::new(&store, &heartbeat)
        .with_principal(CommandPrincipal::new(
            PrincipalKind::Agent,
            trogon_decider_runtime::PrincipalId::new(owner().caller().as_str()).unwrap(),
        ))
        .with_authorizer(&grant)
        .execute()
        .await
        .unwrap();
    let before = store
        .read_stream(ReadStreamRequest {
            stream_id: &peer,
            from: ReadFrom::Beginning,
        })
        .await
        .unwrap();
    let real_heartbeat = before.events.last().unwrap();
    assert!(matches!(
        real_heartbeat
            .decode::<proto::PeerEvent>()
            .unwrap()
            .into_decoded()
            .unwrap()
            .event,
        Some(PeerEventCase::PeerHeartbeatRecorded(_))
    ));
    let padding = 4097usize.checked_sub(before.events.len()).unwrap();
    let events = (0..padding)
        .map(|index| {
            let mut event = real_heartbeat.event.clone();
            event.id = format!("00000000-0000-4000-8000-{index:012x}").parse().unwrap();
            event
        })
        .collect();
    store
        .append_stream(AppendStreamRequest {
            stream_id: &peer,
            stream_write_precondition: before.current_position.into(),
            events,
        })
        .await
        .unwrap();
    let full = store
        .read_stream(ReadStreamRequest {
            stream_id: &peer,
            from: ReadFrom::Beginning,
        })
        .await
        .unwrap();
    assert_eq!(full.events.len(), 4097);
    let mut valid_state = trogon_peer::commands::state::initial_state();
    for event in &full.events {
        let decoded = event.decode::<proto::PeerEvent>().unwrap().into_decoded().unwrap();
        valid_state = trogon_peer::commands::state::evolve(valid_state, &decoded).unwrap();
    }
    assert_eq!(valid_state.enrollment.peer_id, enrolled.peer_id);
    assert_eq!(valid_state.attachment, enrolled.attachment);
    let appends = store.appends.load(Ordering::SeqCst);
    let result = service.enroll(&owner(), request("collaboration_alpha")).await;
    assert!(
        match &result {
            Err(EnrollServiceError::History(trogon_peer_service::HistoryError::Limit)) => true,
            Err(EnrollServiceError::Execution(CommandError::ReplayLimitExceeded(detail)))
            | Err(EnrollServiceError::Attach(CommandError::ReplayLimitExceeded(detail))) => {
                assert_eq!(detail.limit.as_u64(), 4096);
                assert_eq!(detail.replayed_event_count, 4097);
                true
            }
            _ => false,
        },
        "resume must enforce the typed replay ceiling: {result:?}"
    );
    assert_eq!(
        store.appends.load(Ordering::SeqCst),
        appends + 1,
        "only the failing create-only attempt is permitted"
    );
    assert_same_durable_history(
        &(store
            .read_stream(ReadStreamRequest {
                stream_id: &peer,
                from: ReadFrom::Beginning,
            })
            .await
            .unwrap()),
        &full,
    );
}

#[tokio::test]
async fn live_connector_resume_refuses_an_unknown_authority_event_without_appending() {
    let store = ObservedStore::default();
    let service = PeerService::new(store.clone(), policy(), FixedClock(now()));
    let enrolled = service.enroll(&owner(), request("collaboration_alpha")).await.unwrap();
    let peer = PeerId::parse(enrolled.peer_id.clone().unwrap()).unwrap();
    let before = store
        .read_stream(ReadStreamRequest {
            stream_id: &peer,
            from: ReadFrom::Beginning,
        })
        .await
        .unwrap();
    let mut future_authority = before.events[0].event.clone();
    future_authority.id = "00000000-0000-4000-8000-ffffffffffff".parse().unwrap();
    future_authority.r#type = "trogonai.peers.future.PeerAuthorityTransferred".into();
    store
        .append_stream(AppendStreamRequest {
            stream_id: &peer,
            stream_write_precondition: before.current_position.into(),
            events: vec![future_authority],
        })
        .await
        .unwrap();
    let full = store
        .read_stream(ReadStreamRequest {
            stream_id: &peer,
            from: ReadFrom::Beginning,
        })
        .await
        .unwrap();
    assert_eq!(full.events.len(), before.events.len() + 1);
    assert!(
        full.events
            .last()
            .unwrap()
            .decode::<proto::PeerEvent>()
            .unwrap()
            .into_decoded()
            .is_none()
    );
    let appends = store.appends.load(Ordering::SeqCst);
    assert!(matches!(
        service.enroll(&owner(), request("collaboration_alpha")).await,
        Err(EnrollServiceError::History(
            trogon_peer_service::HistoryError::UnknownEvent
        ))
    ));
    assert_eq!(
        store.appends.load(Ordering::SeqCst),
        appends + 1,
        "only the failing create-only attempt is permitted"
    );
    assert_same_durable_history(
        &(store
            .read_stream(ReadStreamRequest {
                stream_id: &peer,
                from: ReadFrom::Beginning,
            })
            .await
            .unwrap()),
        &full,
    );
}

struct ResumeDenied;
impl CommandAuthorizer<Enroll> for ResumeDenied {
    fn authorize(&self, principal: &CommandPrincipal, command: &Enroll) -> Result<(), AuthorizationDeniedError> {
        policy().authorize(principal, command)
    }
}
impl CommandAuthorizer<trogon_peer::Attach> for ResumeDenied {
    fn authorize(
        &self,
        _principal: &CommandPrincipal,
        _command: &trogon_peer::Attach,
    ) -> Result<(), AuthorizationDeniedError> {
        Err(AuthorizationDeniedError::new("resume not granted"))
    }
}
#[tokio::test]
async fn a_create_conflict_cannot_bypass_separate_resume_authorization() {
    let store = ObservedStore::default();
    let enrolled = PeerService::new(store.clone(), policy(), FixedClock(now()))
        .enroll(&owner(), request("collaboration_alpha"))
        .await
        .unwrap();
    let peer = PeerId::parse(enrolled.peer_id.unwrap()).unwrap();
    let original = store
        .read_stream(ReadStreamRequest {
            stream_id: &peer,
            from: ReadFrom::Beginning,
        })
        .await
        .unwrap();
    let reads = store.reads.load(Ordering::SeqCst);
    let denied = PeerService::new(store.clone(), ResumeDenied, FixedClock(now()))
        .enroll(&owner(), request("collaboration_alpha"))
        .await;
    assert!(matches!(
        denied,
        Err(EnrollServiceError::Attach(CommandError::Unauthorized(_)))
    ));
    assert_eq!(
        store.reads.load(Ordering::SeqCst),
        reads,
        "resume denial must precede history access"
    );
    assert_eq!(store.appends.load(Ordering::SeqCst), 2);
    assert_same_durable_history(
        &(store
            .read_stream(ReadStreamRequest {
                stream_id: &peer,
                from: ReadFrom::Beginning,
            })
            .await
            .unwrap()),
        &original,
    );
}

fn assert_same_durable_history(actual: &ReadStreamResponse, expected: &ReadStreamResponse) {
    assert_eq!(actual.current_position, expected.current_position);
    assert_eq!(actual.events.len(), expected.events.len());
    for (actual, expected) in actual.events.iter().zip(&expected.events) {
        assert_eq!(actual.stream_id, expected.stream_id);
        assert_eq!(actual.stream_position, expected.stream_position);
        assert_eq!(actual.event, expected.event);
    }
}

impl CommandAuthorizer<trogon_peer_service::PeerAccess> for EnrollmentPolicy {
    fn authorize(
        &self,
        principal: &CommandPrincipal,
        access: &trogon_peer_service::PeerAccess,
    ) -> Result<(), AuthorizationDeniedError> {
        if access.owner() != &self.owner
            || principal.kind() != PrincipalKind::Agent
            || principal.id().as_str() != self.owner.caller().as_str()
            || self.allowed_peer.as_ref().is_some_and(|peer| peer != access.peer())
            || self
                .allowed_operation
                .is_some_and(|operation| operation != access.operation())
        {
            return Err(AuthorizationDeniedError::new("outside owner grant"));
        }
        Ok(())
    }
}
#[tokio::test]
async fn heartbeat_renews_then_detach_is_idempotent_and_old_generation_is_fenced() {
    let store = ObservedStore::default();
    let service = PeerService::new(store.clone(), policy(), FixedClock(now()));
    let first = service.enroll(&owner(), request("collaboration_alpha")).await.unwrap();
    let reference = first.attachment.attachment_ref.clone();
    let later = PeerService::new(
        store.clone(),
        policy(),
        FixedClock(now() + chrono::Duration::seconds(2)),
    );
    let renewed = later
        .heartbeat(
            &owner(),
            proto::HeartbeatRequest {
                attachment_ref: reference.clone(),
                availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_BUSY.into()),
                ..Default::default()
            },
        )
        .await;
    assert!(renewed.is_ok(), "heartbeat should renew: {renewed:?}");
    assert_eq!(
        renewed.unwrap().attachment.expires_at.as_option(),
        Some(&timestamp_from_datetime(&(now() + chrono::Duration::seconds(12))))
    );
    later
        .detach(
            &owner(),
            proto::DetachRequest {
                attachment_ref: reference.clone(),
            },
        )
        .await
        .unwrap();
    let appends = store.appends.load(Ordering::SeqCst);
    later
        .detach(
            &owner(),
            proto::DetachRequest {
                attachment_ref: reference.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(store.appends.load(Ordering::SeqCst), appends);
    let replacement = later.enroll(&owner(), request("collaboration_alpha")).await.unwrap();
    assert_eq!(replacement.attachment.attachment_ref.generation, Some(2));
    assert!(
        later
            .heartbeat(
                &owner(),
                proto::HeartbeatRequest {
                    attachment_ref: reference,
                    availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_READY.into()),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
}

impl CommandAuthorizer<trogon_peer::Heartbeat> for EnrollmentPolicy {
    fn authorize(
        &self,
        principal: &CommandPrincipal,
        command: &trogon_peer::Heartbeat,
    ) -> Result<(), AuthorizationDeniedError> {
        self.authorize(
            principal,
            &trogon_peer_service::PeerAccess::new(
                command.owner().clone(),
                command.attachment().peer_id().clone(),
                trogon_peer_service::PeerOperation::Heartbeat,
            ),
        )
    }
}
impl CommandAuthorizer<trogon_peer::Detach> for EnrollmentPolicy {
    fn authorize(
        &self,
        principal: &CommandPrincipal,
        command: &trogon_peer::Detach,
    ) -> Result<(), AuthorizationDeniedError> {
        self.authorize(
            principal,
            &trogon_peer_service::PeerAccess::new(
                command.owner().clone(),
                command.attachment().peer_id().clone(),
                trogon_peer_service::PeerOperation::Detach,
            ),
        )
    }
}

#[tokio::test]
async fn a_future_generation_cannot_detach_the_current_attachment() {
    let store = ObservedStore::default();
    let service = PeerService::new(store.clone(), policy(), FixedClock(now()));
    let response = service.enroll(&owner(), request("collaboration_alpha")).await.unwrap();
    let reference = response.attachment.attachment_ref.clone();
    let mut future = reference.clone();
    future.as_option_mut().expect("enrolled attachment").generation = Some(2);
    let appends = store.appends.load(Ordering::SeqCst);
    assert!(matches!(
        service
            .detach(&owner(), proto::DetachRequest { attachment_ref: future })
            .await,
        Err(trogon_peer_service::PeerApplicationError::Detach(CommandError::Decide(
            _
        )))
    ));
    assert_eq!(store.appends.load(Ordering::SeqCst), appends);
    let heartbeat = service
        .heartbeat(
            &owner(),
            proto::HeartbeatRequest {
                attachment_ref: reference,
                availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_READY.into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        heartbeat.attachment.status.and_then(|v| v.as_known()),
        Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED)
    );
}

#[tokio::test]
async fn second_live_connector_is_refused_and_different_native_identity_gets_a_new_peer() {
    let store = ObservedStore::default();
    let service = PeerService::new(store.clone(), policy(), FixedClock(now()));
    let first = service.enroll(&owner(), request("collaboration_alpha")).await.unwrap();
    let mut competing = request("collaboration_alpha");
    competing.connector_id = Some("other_connector".into());
    assert!(matches!(
        service.enroll(&owner(), competing).await,
        Err(EnrollServiceError::Attach(CommandError::Decide(
            trogon_peer::AttachError::InUse
        )))
    ));
    let mut other_session = request("collaboration_alpha");
    other_session.native_session_id = Some("other_native_session".into());
    let different = service.enroll(&owner(), other_session).await.unwrap();
    assert_ne!(first.peer_id, different.peer_id);
    assert_eq!(first.attachment.attachment_ref.generation, Some(1));
    assert_eq!(different.attachment.attachment_ref.generation, Some(1));
}

#[tokio::test]
async fn resume_does_not_return_a_competing_connector_that_won_during_the_response_read() {
    let store = ObservedStore::default();
    let service = PeerService::new(store.clone(), policy(), FixedClock(now()));
    let first = service.enroll(&owner(), request("collaboration_alpha")).await.unwrap();
    let other = ObservedStore {
        inner: store.inner.clone(),
        ..Default::default()
    };
    let action = async move {
        let competing = PeerService::new(other, policy(), FixedClock(now()));
        competing
            .detach(
                &owner(),
                proto::DetachRequest {
                    attachment_ref: first.attachment.attachment_ref.clone(),
                },
            )
            .await
            .unwrap();
        let mut request = request("collaboration_alpha");
        request.connector_id = Some("competing_connector".into());
        competing.enroll(&owner(), request).await.unwrap();
    };
    *store.before_read.lock().unwrap() = Some((store.reads.load(Ordering::SeqCst) + 3, Box::pin(action)));
    let resumed = service.enroll(&owner(), request("collaboration_alpha")).await;
    assert!(
        matches!(
            resumed,
            Err(EnrollServiceError::ResumeGuard(trogon_peer::HeartbeatError::Fenced))
        ),
        "resume must refuse a connector that changed after the decision: {resumed:?}"
    );
    assert!(
        store.before_read.lock().unwrap().is_none(),
        "competing connector interleave must execute"
    );
}

#[tokio::test]
async fn heartbeat_grant_cannot_authorize_another_peer_or_a_detach_operation() {
    let store = ObservedStore::default();
    let enrollment = PeerService::new(store.clone(), policy(), FixedClock(now()))
        .enroll(&owner(), request("collaboration_alpha"))
        .await
        .unwrap();
    let reference = enrollment.attachment.attachment_ref.clone();
    let mut grant = policy();
    grant.allowed_peer = Some(PeerId::parse(enrollment.peer_id.unwrap()).unwrap());
    grant.allowed_operation = Some(PeerOperation::Heartbeat);
    let service = PeerService::new(store.clone(), grant, FixedClock(now() + chrono::Duration::seconds(1)));
    service
        .heartbeat(
            &owner(),
            proto::HeartbeatRequest {
                attachment_ref: reference.clone(),
                availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_READY.into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let reads = store.reads.load(Ordering::SeqCst);
    let appends = store.appends.load(Ordering::SeqCst);
    assert!(matches!(
        service
            .detach(
                &owner(),
                proto::DetachRequest {
                    attachment_ref: reference.clone()
                }
            )
            .await,
        Err(PeerApplicationError::Detach(CommandError::Unauthorized(_)))
    ));
    let mut foreign = reference;
    foreign.as_option_mut().unwrap().peer_id = Some("f".repeat(64));
    assert!(matches!(
        service
            .heartbeat(
                &owner(),
                proto::HeartbeatRequest {
                    attachment_ref: foreign,
                    availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_READY.into()),
                    ..Default::default()
                }
            )
            .await,
        Err(PeerApplicationError::Heartbeat(CommandError::Unauthorized(_)))
    ));
    assert_eq!(store.reads.load(Ordering::SeqCst), reads);
    assert_eq!(store.appends.load(Ordering::SeqCst), appends);
}

async fn expired_resume_unknown_authority_history() -> (ObservedStore, PeerId, ReadStreamResponse, DateTime<Utc>) {
    let store = ObservedStore::default();
    let enrolled = PeerService::new(store.clone(), policy(), FixedClock(now()))
        .enroll(&owner(), request("collaboration_alpha"))
        .await
        .unwrap();
    let peer = PeerId::parse(enrolled.peer_id.clone().unwrap()).unwrap();
    let expiry =
        trogonai_proto::convert::datetime_from_timestamp(enrolled.attachment.expires_at.as_option().unwrap()).unwrap();
    let original = store
        .read_stream(ReadStreamRequest {
            stream_id: &peer,
            from: ReadFrom::Beginning,
        })
        .await
        .unwrap();
    let mut future_authority = original.events[0].event.clone();
    future_authority.id = "00000000-0000-4000-8000-fffffffffff0".parse().unwrap();
    future_authority.r#type = "trogonai.peers.future.PeerAuthorityTransferred".into();
    store
        .append_stream(AppendStreamRequest {
            stream_id: &peer,
            stream_write_precondition: original.current_position.into(),
            events: vec![future_authority],
        })
        .await
        .unwrap();
    let history = store
        .read_stream(ReadStreamRequest {
            stream_id: &peer,
            from: ReadFrom::Beginning,
        })
        .await
        .unwrap();
    assert_eq!(history.events.len(), original.events.len() + 1);
    assert!(
        history
            .events
            .last()
            .unwrap()
            .decode::<proto::PeerEvent>()
            .unwrap()
            .into_decoded()
            .is_none()
    );
    (store, peer, history, expiry)
}

#[tokio::test]
async fn expired_connector_replacement_refuses_unknown_authority_history_without_a_new_attachment() {
    let (store, peer, history, expiry) = expired_resume_unknown_authority_history().await;
    let mut replacement = request("collaboration_alpha");
    replacement.connector_id = Some("replacement_alpha".into());
    let appends = store.appends.load(Ordering::SeqCst);
    let result = PeerService::new(store.clone(), policy(), FixedClock(expiry))
        .enroll(&owner(), replacement)
        .await;
    assert!(
        matches!(
            &result,
            Err(EnrollServiceError::History(
                trogon_peer_service::HistoryError::UnknownEvent
            ))
        ),
        "expired replacement cannot infer authority across an unknown event: {result:?}"
    );
    assert_eq!(
        store.appends.load(Ordering::SeqCst),
        appends + 1,
        "only the rejected create-only attempt may reach the append port"
    );
    assert_same_durable_history(
        &store
            .read_stream(ReadStreamRequest {
                stream_id: &peer,
                from: ReadFrom::Beginning,
            })
            .await
            .unwrap(),
        &history,
    );
}

#[tokio::test]
async fn denied_expired_replacement_does_not_read_unknown_authority_history() {
    let (store, peer, history, expiry) = expired_resume_unknown_authority_history().await;
    let mut replacement = request("collaboration_alpha");
    replacement.connector_id = Some("replacement_alpha".into());
    let reads = store.reads.load(Ordering::SeqCst);
    let appends = store.appends.load(Ordering::SeqCst);
    let result = PeerService::new(store.clone(), ResumeDenied, FixedClock(expiry))
        .enroll(&owner(), replacement)
        .await;
    assert!(matches!(
        result,
        Err(EnrollServiceError::Attach(CommandError::Unauthorized(_)))
    ));
    assert_eq!(store.reads.load(Ordering::SeqCst), reads);
    assert_eq!(store.appends.load(Ordering::SeqCst), appends + 1);
    assert_same_durable_history(
        &store
            .read_stream(ReadStreamRequest {
                stream_id: &peer,
                from: ReadFrom::Beginning,
            })
            .await
            .unwrap(),
        &history,
    );
}

#[derive(Clone)]
struct UnknownAuthorityAfterHistoryRead {
    inner: ObservedStore,
    armed: Arc<std::sync::atomic::AtomicBool>,
    injected_history: Arc<Mutex<Option<ReadStreamResponse>>>,
}

impl UnknownAuthorityAfterHistoryRead {
    async fn inject_after_snapshot(&self, peer: &PeerId, snapshot: ReadStreamResponse) -> ReadStreamResponse {
        if self.armed.swap(false, Ordering::SeqCst) {
            let mut unknown = snapshot.events.first().expect("enrolled snapshot").event.clone();
            unknown.id = "00000000-0000-4000-8000-fffffffffff1".parse().unwrap();
            unknown.r#type = "trogonai.peers.future.PeerAuthorityTransferred".into();
            self.inner
                .append_stream(AppendStreamRequest {
                    stream_id: peer,
                    stream_write_precondition: snapshot.current_position.into(),
                    events: vec![unknown],
                })
                .await
                .expect("append the competing authority envelope at its actual OCC position");
            let changed = self
                .inner
                .read_stream(ReadStreamRequest {
                    stream_id: peer,
                    from: ReadFrom::Beginning,
                })
                .await
                .unwrap();
            assert!(
                changed
                    .events
                    .last()
                    .unwrap()
                    .decode::<proto::PeerEvent>()
                    .unwrap()
                    .into_decoded()
                    .is_none()
            );
            *self.injected_history.lock().unwrap() = Some(changed);
        }
        snapshot
    }
}

impl StreamRead<PeerId> for UnknownAuthorityAfterHistoryRead {
    type Error = <ObservedStore as StreamRead<PeerId>>::Error;

    async fn read_stream(&self, request: ReadStreamRequest<'_, PeerId>) -> Result<ReadStreamResponse, Self::Error> {
        let peer = request.stream_id.clone();
        let snapshot = self.inner.read_stream(request).await?;
        Ok(self.inject_after_snapshot(&peer, snapshot).await)
    }

    async fn read_stream_bounded(
        &self,
        request: ReadStreamRequest<'_, PeerId>,
        maximum: u64,
    ) -> Result<ReadStreamResponse, Self::Error> {
        let peer = request.stream_id.clone();
        let snapshot = self.inner.read_stream_bounded(request, maximum).await?;
        Ok(self.inject_after_snapshot(&peer, snapshot).await)
    }
}

impl StreamAppend<PeerId> for UnknownAuthorityAfterHistoryRead {
    type Error = <ObservedStore as StreamAppend<PeerId>>::Error;

    async fn append_stream(
        &self,
        request: AppendStreamRequest<'_, PeerId>,
    ) -> Result<AppendStreamResponse, Self::Error> {
        self.inner.append_stream(request).await
    }

    fn classify_append_failure(&self, error: &Self::Error) -> AppendFailure {
        self.inner.classify_append_failure(error)
    }
}

#[tokio::test]
async fn expired_replacement_cannot_append_after_authority_changes_past_its_validated_history_position() {
    let inner = ObservedStore::default();
    let first = PeerService::new(inner.clone(), policy(), FixedClock(now()))
        .enroll(&owner(), request("collaboration_alpha"))
        .await
        .unwrap();
    let peer = PeerId::parse(first.peer_id.clone().unwrap()).unwrap();
    let expiry =
        trogonai_proto::convert::datetime_from_timestamp(first.attachment.expires_at.as_option().unwrap()).unwrap();
    let store = UnknownAuthorityAfterHistoryRead {
        inner: inner.clone(),
        armed: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        injected_history: Arc::new(Mutex::new(None)),
    };
    let mut replacement = request("collaboration_alpha");
    replacement.connector_id = Some("replacement_raced".into());
    let appends = inner.appends.load(Ordering::SeqCst);
    let result = PeerService::new(store.clone(), policy(), FixedClock(expiry))
        .enroll(&owner(), replacement)
        .await;
    assert!(
        matches!(
            &result,
            Err(EnrollServiceError::Attach(CommandError::Append(error)))
                if store.classify_append_failure(error) == AppendFailure::WriteConflict
        ),
        "the validated position must fence a competing unknown authority event: {result:?}"
    );
    assert!(!store.armed.load(Ordering::SeqCst));
    assert_eq!(
        inner.appends.load(Ordering::SeqCst),
        appends + 3,
        "create rejection, competing envelope, and one fenced attachment attempt are the only append calls"
    );
    let injected = store
        .injected_history
        .lock()
        .unwrap()
        .clone()
        .expect("the competing envelope was persisted");
    let actual = inner
        .read_stream(ReadStreamRequest {
            stream_id: &peer,
            from: ReadFrom::Beginning,
        })
        .await
        .unwrap();
    assert_same_durable_history(&actual, &injected);
}

#[tokio::test]
async fn live_resume_rechecks_unknown_authority_that_raced_its_validated_history() {
    let inner = ObservedStore::default();
    let first = PeerService::new(inner.clone(), policy(), FixedClock(now()))
        .enroll(&owner(), request("collaboration_alpha"))
        .await
        .unwrap();
    let peer = PeerId::parse(first.peer_id.clone().unwrap()).unwrap();
    let store = UnknownAuthorityAfterHistoryRead {
        inner: inner.clone(),
        armed: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        injected_history: Arc::new(Mutex::new(None)),
    };
    let appends = inner.appends.load(Ordering::SeqCst);
    let result = PeerService::new(store.clone(), policy(), FixedClock(now()))
        .enroll(&owner(), request("collaboration_alpha"))
        .await;
    assert!(matches!(
        result,
        Err(EnrollServiceError::History(
            trogon_peer_service::HistoryError::UnknownEvent
        ))
    ));
    assert!(!store.armed.load(Ordering::SeqCst));
    assert_eq!(
        inner.appends.load(Ordering::SeqCst),
        appends + 2,
        "only the rejected create and competing authority envelope may append"
    );
    let injected = store
        .injected_history
        .lock()
        .unwrap()
        .clone()
        .expect("persisted competing envelope");
    let actual = inner
        .read_stream(ReadStreamRequest {
            stream_id: &peer,
            from: ReadFrom::Beginning,
        })
        .await
        .unwrap();
    assert_same_durable_history(&actual, &injected);
}
