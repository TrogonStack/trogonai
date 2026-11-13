use std::sync::{Arc, Mutex};

use async_nats::jetstream::message::{OutboundMessage, StreamMessage};
use async_nats::jetstream::stream::{LastRawMessageError, LastRawMessageErrorKind, RawMessageError};
use trogon_credential_domain::{CredentialIdentity, CredentialKind, CredentialOwnerId, CredentialScope, SourceKind};
use trogon_decider_runtime::{EventEncode, EventType};
use trogon_nats::jetstream::{JetStreamPublishMessage, MockJetStreamPublishMessage};
use trogonai_proto::gateway::credentials::CredentialStateSnapshotCase;

use super::*;
use crate::fixtures::credential_events::{
    activated, append, destroy_requested, destroyed, message, raw_message, revoked, write_requested,
};
use crate::fixtures::identity;

/// Another credential of the same project, on a subject of its own.
fn neighbour() -> CredentialIdentity {
    CredentialIdentity::derive(
        CredentialScope::source(
            CredentialOwnerId::new(crate::fixtures::OWNER).unwrap(),
            SourceKind::GitHub,
        ),
        CredentialKind::SigningSecret,
    )
}

fn state_case(state: &CredentialStateSnapshot) -> &CredentialStateSnapshotCase {
    state.state.as_ref().unwrap()
}

async fn stream_with(messages: impl IntoIterator<Item = OutboundMessage>) -> MockJetStreamPublishMessage {
    let stream = MockJetStreamPublishMessage::new();
    append(&stream, messages).await;
    stream
}

/// A stream whose reads fail, or answer what the test scripts.
#[derive(Clone, Default)]
struct ScriptedStream {
    last: Option<Result<u64, LastRawMessageErrorKind>>,
    first: Option<Result<StreamMessage, LastRawMessageErrorKind>>,
}

fn stream_message(sequence: u64) -> StreamMessage {
    StreamMessage {
        subject: "credentials.events.v1.project-1.unused".into(),
        sequence,
        headers: async_nats::HeaderMap::new(),
        payload: Default::default(),
        time: time::OffsetDateTime::UNIX_EPOCH,
    }
}

impl JetStreamLastRawMessageBySubject for ScriptedStream {
    async fn get_last_raw_message_by_subject(&self, _subject: &str) -> Result<StreamMessage, LastRawMessageError> {
        match self.last.clone().unwrap() {
            Ok(sequence) => Ok(stream_message(sequence)),
            Err(kind) => Err(LastRawMessageError::new(kind)),
        }
    }
}

impl JetStreamFirstRawMessageBySubject for ScriptedStream {
    async fn get_first_raw_message_by_subject(
        &self,
        _subject: &str,
        _sequence: u64,
    ) -> Result<StreamMessage, RawMessageError> {
        self.first.clone().unwrap().map_err(RawMessageError::new)
    }
}

/// A stream that takes one more event, through `publisher`, right after the
/// read fixed its bound.
#[derive(Clone)]
struct GrowingStream<S, P> {
    inner: S,
    publisher: P,
    late: Arc<Mutex<Option<OutboundMessage>>>,
}

impl<S, P> GrowingStream<S, P> {
    fn new(inner: S, publisher: P, late: OutboundMessage) -> Self {
        Self {
            inner,
            publisher,
            late: Arc::new(Mutex::new(Some(late))),
        }
    }
}

impl<S, P> JetStreamLastRawMessageBySubject for GrowingStream<S, P>
where
    S: JetStreamLastRawMessageBySubject + Sync,
    P: JetStreamPublishMessage,
    P::PublishError: std::fmt::Debug,
{
    async fn get_last_raw_message_by_subject(&self, subject: &str) -> Result<StreamMessage, LastRawMessageError> {
        let bound = self.inner.get_last_raw_message_by_subject(subject).await;
        let late = self.late.lock().unwrap().take();
        append(&self.publisher, late).await;
        bound
    }
}

impl<S, P> JetStreamFirstRawMessageBySubject for GrowingStream<S, P>
where
    S: JetStreamFirstRawMessageBySubject,
    P: Send + Sync + 'static,
{
    async fn get_first_raw_message_by_subject(
        &self,
        subject: &str,
        sequence: u64,
    ) -> Result<StreamMessage, RawMessageError> {
        self.inner.get_first_raw_message_by_subject(subject, sequence).await
    }
}

#[tokio::test]
async fn reports_the_stream_unavailable_when_its_end_cannot_be_read() {
    let states = JetStreamCredentialStates::new(ScriptedStream {
        last: Some(Err(LastRawMessageErrorKind::Other)),
        ..ScriptedStream::default()
    });

    let error = states.read(&identity().key()).await.unwrap_err();

    assert!(
        matches!(error, CredentialStateReadError::Unavailable { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn reports_the_stream_unavailable_when_an_event_cannot_be_read() {
    let states = JetStreamCredentialStates::new(ScriptedStream {
        last: Some(Ok(3)),
        first: Some(Err(LastRawMessageErrorKind::Other)),
    });

    let error = states.read(&identity().key()).await.unwrap_err();

    assert!(
        matches!(error, CredentialStateReadError::Unavailable { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn reports_the_stream_unavailable_when_the_bounding_event_is_gone() {
    let removed_before_the_read = ScriptedStream {
        last: Some(Ok(3)),
        first: Some(Err(LastRawMessageErrorKind::NoMessageFound)),
    };
    let removed_past_the_bound = ScriptedStream {
        last: Some(Ok(3)),
        first: Some(Ok(stream_message(4))),
    };

    let removed_before = JetStreamCredentialStates::new(removed_before_the_read)
        .read(&identity().key())
        .await
        .unwrap_err();
    let removed_past = JetStreamCredentialStates::new(removed_past_the_bound)
        .read(&identity().key())
        .await
        .unwrap_err();

    assert!(
        matches!(removed_before, CredentialStateReadError::Unavailable { .. }),
        "{removed_before:?}"
    );
    assert!(
        matches!(removed_past, CredentialStateReadError::Unavailable { .. }),
        "{removed_past:?}"
    );
}

#[tokio::test]
async fn refuses_to_fold_an_event_whose_payload_does_not_decode() {
    let identity = identity();
    let corrupt = raw_message(
        &identity,
        Some(revoked(&identity).event_type().unwrap()),
        vec![0xff, 0xff],
    );
    let stream = stream_with([
        message(&identity, &write_requested(&identity)),
        message(&identity, &activated(&identity)),
        corrupt,
    ])
    .await;

    let error = JetStreamCredentialStates::new(stream)
        .read(&identity.key())
        .await
        .unwrap_err();

    assert!(
        matches!(error, CredentialStateReadError::Undecodable { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn refuses_to_fold_an_event_type_it_does_not_know() {
    let identity = identity();
    let stream = stream_with([
        message(&identity, &write_requested(&identity)),
        message(&identity, &activated(&identity)),
        raw_message(&identity, Some("trogon.credential.suspended.v1"), Vec::new()),
    ])
    .await;

    let error = JetStreamCredentialStates::new(stream)
        .read(&identity.key())
        .await
        .unwrap_err();

    assert!(
        matches!(error, CredentialStateReadError::Undecodable { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn refuses_to_fold_an_event_without_its_type() {
    let identity = identity();
    let stream = stream_with([raw_message(
        &identity,
        None,
        write_requested(&identity).encode().unwrap(),
    )])
    .await;

    let error = JetStreamCredentialStates::new(stream)
        .read(&identity.key())
        .await
        .unwrap_err();

    assert!(
        matches!(error, CredentialStateReadError::Undecodable { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn refuses_to_fold_an_event_of_another_credential_filed_on_its_subject() {
    let identity = identity();
    let misfiled = raw_message(
        &identity,
        Some(write_requested(&neighbour()).event_type().unwrap()),
        write_requested(&neighbour()).encode().unwrap(),
    );
    let stream = stream_with([misfiled]).await;

    let error = JetStreamCredentialStates::new(stream)
        .read(&identity.key())
        .await
        .unwrap_err();

    assert!(
        matches!(error, CredentialStateReadError::Undecodable { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn refuses_to_fold_events_out_of_order() {
    let identity = identity();
    let stream = stream_with([message(&identity, &activated(&identity))]).await;

    let error = JetStreamCredentialStates::new(stream)
        .read(&identity.key())
        .await
        .unwrap_err();

    assert!(
        matches!(error, CredentialStateReadError::Undecodable { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn knows_nothing_of_a_credential_without_events() {
    let stream = stream_with([message(&neighbour(), &write_requested(&neighbour()))]).await;

    let state = JetStreamCredentialStates::new(stream)
        .read(&identity().key())
        .await
        .unwrap();

    assert_eq!(state, None);
}

#[tokio::test]
async fn folds_an_activated_credential_without_its_neighbours_events() {
    let identity = identity();
    let stream = stream_with([
        message(&identity, &write_requested(&identity)),
        message(&neighbour(), &write_requested(&neighbour())),
        message(&identity, &activated(&identity)),
        message(&neighbour(), &activated(&neighbour())),
        message(&neighbour(), &revoked(&neighbour())),
    ])
    .await;

    let state = JetStreamCredentialStates::new(stream)
        .read(&identity.key())
        .await
        .unwrap()
        .unwrap();

    assert!(
        matches!(state_case(&state), CredentialStateSnapshotCase::Active(_)),
        "{state:?}"
    );
}

#[tokio::test]
async fn folds_a_revocation_committed_after_activation() {
    let identity = identity();
    let stream = stream_with([
        message(&identity, &write_requested(&identity)),
        message(&identity, &activated(&identity)),
        message(&identity, &revoked(&identity)),
    ])
    .await;

    let state = JetStreamCredentialStates::new(stream)
        .read(&identity.key())
        .await
        .unwrap()
        .unwrap();

    assert!(
        matches!(state_case(&state), CredentialStateSnapshotCase::Revoked(_)),
        "{state:?}"
    );
}

#[tokio::test]
async fn folds_a_destroyed_credential() {
    let identity = identity();
    let stream = stream_with([
        message(&identity, &write_requested(&identity)),
        message(&identity, &activated(&identity)),
        message(&identity, &revoked(&identity)),
        message(&identity, &destroy_requested(&identity)),
        message(&identity, &destroyed(&identity)),
    ])
    .await;

    let state = JetStreamCredentialStates::new(stream)
        .read(&identity.key())
        .await
        .unwrap()
        .unwrap();

    assert!(
        matches!(state_case(&state), CredentialStateSnapshotCase::Destroyed(_)),
        "{state:?}"
    );
}

#[tokio::test]
async fn stops_at_the_last_event_the_subject_held_when_the_read_started() {
    let identity = identity();
    let inner = stream_with([
        message(&identity, &write_requested(&identity)),
        message(&identity, &activated(&identity)),
    ])
    .await;
    let stream = GrowingStream::new(inner.clone(), inner.clone(), message(&identity, &revoked(&identity)));

    let state = JetStreamCredentialStates::new(stream)
        .read(&identity.key())
        .await
        .unwrap()
        .unwrap();

    assert!(
        matches!(state_case(&state), CredentialStateSnapshotCase::Active(_)),
        "{state:?}"
    );
    assert_eq!(inner.published_messages().len(), 3, "the late revocation was appended");
}

mod jetstream_tests;
