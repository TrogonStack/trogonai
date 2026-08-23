use buffa::Message as _;
use trogon_decider::{EventData, EventDecode, EventDecodeOutcome, EventEncode, EventPayloadError, EventType};

use super::{SessionEventCase, v1alpha1};
use crate::codec::{decode_event_case, event_type};

#[cfg(feature = "runtime-host")]
use trogon_decider_runtime::EventIdentity;

pub type SessionEventPayloadError = EventPayloadError<buffa::DecodeError>;

impl EventEncode for v1alpha1::SessionEvent {
    type Error = SessionEventPayloadError;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        self.event
            .as_ref()
            .map(encode_session_event_case)
            .ok_or(SessionEventPayloadError::MissingEvent)
    }
}

impl EventDecode for v1alpha1::SessionEvent {
    type Error = SessionEventPayloadError;

    fn decode(event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
        match decode_session_event_case(event)? {
            Some(event) => Ok(EventDecodeOutcome::Decoded(v1alpha1::SessionEvent {
                event: Some(event),
            })),
            None => Ok(EventDecodeOutcome::Skipped),
        }
    }
}

impl EventType for v1alpha1::SessionEvent {
    type Error = SessionEventPayloadError;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        self.event
            .as_ref()
            .map(session_event_case_type)
            .ok_or(SessionEventPayloadError::MissingEvent)
    }
}

#[cfg(feature = "runtime-host")]
impl EventIdentity for v1alpha1::SessionEvent {}

fn encode_session_event_case(event: &SessionEventCase) -> Vec<u8> {
    match event {
        SessionEventCase::SessionStarted(inner) => inner.encode_to_vec(),
        SessionEventCase::SessionClosed(inner) => inner.encode_to_vec(),
        SessionEventCase::SessionCancelled(inner) => inner.encode_to_vec(),
        SessionEventCase::SessionFailed(inner) => inner.encode_to_vec(),
        SessionEventCase::SessionHidden(inner) => inner.encode_to_vec(),
        SessionEventCase::SessionForked(inner) => inner.encode_to_vec(),
        SessionEventCase::SessionRecovered(inner) => inner.encode_to_vec(),
        SessionEventCase::SessionRewound(inner) => inner.encode_to_vec(),
        SessionEventCase::Compacted(inner) => inner.encode_to_vec(),
        SessionEventCase::UserMessageRecorded(inner) => inner.encode_to_vec(),
        SessionEventCase::AssistantMessageStarted(inner) => inner.encode_to_vec(),
        SessionEventCase::AssistantMessageCompleted(inner) => inner.encode_to_vec(),
        SessionEventCase::AssistantMessageFailed(inner) => inner.encode_to_vec(),
        SessionEventCase::ProviderToolIntentRejected(inner) => inner.encode_to_vec(),
        SessionEventCase::ToolCallRequested(inner) => inner.encode_to_vec(),
        SessionEventCase::ToolCallApproved(inner) => inner.encode_to_vec(),
        SessionEventCase::ToolCallDenied(inner) => inner.encode_to_vec(),
        SessionEventCase::ToolCallStarted(inner) => inner.encode_to_vec(),
        SessionEventCase::ToolCallCompleted(inner) => inner.encode_to_vec(),
        SessionEventCase::ToolCallFailed(inner) => inner.encode_to_vec(),
        SessionEventCase::ArtifactRecorded(inner) => inner.encode_to_vec(),
        SessionEventCase::FileChanged(inner) => inner.encode_to_vec(),
        SessionEventCase::ExecutionAttemptStarted(inner) => inner.encode_to_vec(),
        SessionEventCase::ExecutionAttemptReady(inner) => inner.encode_to_vec(),
        SessionEventCase::ExecutionAttemptEnded(inner) => inner.encode_to_vec(),
        SessionEventCase::CheckpointProduced(inner) => inner.encode_to_vec(),
        SessionEventCase::DelegationDispatched(inner) => inner.encode_to_vec(),
        SessionEventCase::ParentLinked(inner) => inner.encode_to_vec(),
        SessionEventCase::ParentTerminated(inner) => inner.encode_to_vec(),
        SessionEventCase::DelegationDetached(inner) => inner.encode_to_vec(),
        SessionEventCase::ParentHistoryInvalidated(inner) => inner.encode_to_vec(),
        SessionEventCase::ParentDetached(inner) => inner.encode_to_vec(),
        SessionEventCase::ExternalDelegationDispatched(inner) => inner.encode_to_vec(),
        SessionEventCase::OperationReserved(inner) => inner.encode_to_vec(),
        SessionEventCase::OperationOutcomeRecorded(inner) => inner.encode_to_vec(),
        SessionEventCase::OperationCancellationRequested(inner) => inner.encode_to_vec(),
        SessionEventCase::ArtifactErased(inner) => inner.encode_to_vec(),
        SessionEventCase::RedactionApplied(inner) => inner.encode_to_vec(),
        SessionEventCase::SystemNoticeRecorded(inner) => inner.encode_to_vec(),
        SessionEventCase::TodoUpdated(inner) => inner.encode_to_vec(),
        SessionEventCase::SessionRenamed(inner) => inner.encode_to_vec(),
        SessionEventCase::SessionArchived(inner) => inner.encode_to_vec(),
        SessionEventCase::SessionUnarchived(inner) => inner.encode_to_vec(),
    }
}

fn decode_session_event_case(event: EventData<'_>) -> Result<Option<SessionEventCase>, SessionEventPayloadError> {
    let Some(event) = decode_event_case::<v1alpha1::SessionStarted, SessionEventCase>(&event)
        .or_else(|| decode_event_case::<v1alpha1::SessionClosed, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::SessionCancelled, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::SessionFailed, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::SessionHidden, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::SessionForked, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::SessionRecovered, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::SessionRewound, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::Compacted, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::UserMessageRecorded, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::AssistantMessageStarted, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::AssistantMessageCompleted, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::AssistantMessageFailed, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::ToolCallRequested, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::ToolCallApproved, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::ToolCallDenied, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::ToolCallStarted, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::ToolCallCompleted, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::ToolCallFailed, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::ArtifactRecorded, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::FileChanged, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::ExecutionAttemptStarted, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::ExecutionAttemptReady, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::ExecutionAttemptEnded, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::CheckpointProduced, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::DelegationDispatched, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::ParentLinked, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::ParentTerminated, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::DelegationDetached, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::ParentHistoryInvalidated, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::ParentDetached, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::ExternalDelegationDispatched, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::OperationReserved, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::OperationOutcomeRecorded, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::OperationCancellationRequested, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::ArtifactErased, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::RedactionApplied, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::SystemNoticeRecorded, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::TodoUpdated, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::SessionRenamed, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::SessionArchived, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::SessionUnarchived, SessionEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::ProviderToolIntentRejected, SessionEventCase>(&event))
    else {
        return Ok(None);
    };

    event.map(Some).map_err(SessionEventPayloadError::Decode)
}

fn session_event_case_type(event: &SessionEventCase) -> &'static str {
    match event {
        SessionEventCase::SessionStarted(_) => event_type::<v1alpha1::SessionStarted>(),
        SessionEventCase::SessionClosed(_) => event_type::<v1alpha1::SessionClosed>(),
        SessionEventCase::SessionCancelled(_) => event_type::<v1alpha1::SessionCancelled>(),
        SessionEventCase::SessionFailed(_) => event_type::<v1alpha1::SessionFailed>(),
        SessionEventCase::SessionHidden(_) => event_type::<v1alpha1::SessionHidden>(),
        SessionEventCase::SessionForked(_) => event_type::<v1alpha1::SessionForked>(),
        SessionEventCase::SessionRecovered(_) => event_type::<v1alpha1::SessionRecovered>(),
        SessionEventCase::SessionRewound(_) => event_type::<v1alpha1::SessionRewound>(),
        SessionEventCase::Compacted(_) => event_type::<v1alpha1::Compacted>(),
        SessionEventCase::UserMessageRecorded(_) => event_type::<v1alpha1::UserMessageRecorded>(),
        SessionEventCase::AssistantMessageStarted(_) => event_type::<v1alpha1::AssistantMessageStarted>(),
        SessionEventCase::AssistantMessageCompleted(_) => event_type::<v1alpha1::AssistantMessageCompleted>(),
        SessionEventCase::AssistantMessageFailed(_) => event_type::<v1alpha1::AssistantMessageFailed>(),
        SessionEventCase::ToolCallRequested(_) => event_type::<v1alpha1::ToolCallRequested>(),
        SessionEventCase::ToolCallApproved(_) => event_type::<v1alpha1::ToolCallApproved>(),
        SessionEventCase::ToolCallDenied(_) => event_type::<v1alpha1::ToolCallDenied>(),
        SessionEventCase::ToolCallStarted(_) => event_type::<v1alpha1::ToolCallStarted>(),
        SessionEventCase::ToolCallCompleted(_) => event_type::<v1alpha1::ToolCallCompleted>(),
        SessionEventCase::ToolCallFailed(_) => event_type::<v1alpha1::ToolCallFailed>(),
        SessionEventCase::ArtifactRecorded(_) => event_type::<v1alpha1::ArtifactRecorded>(),
        SessionEventCase::FileChanged(_) => event_type::<v1alpha1::FileChanged>(),
        SessionEventCase::ExecutionAttemptStarted(_) => event_type::<v1alpha1::ExecutionAttemptStarted>(),
        SessionEventCase::ExecutionAttemptReady(_) => event_type::<v1alpha1::ExecutionAttemptReady>(),
        SessionEventCase::ExecutionAttemptEnded(_) => event_type::<v1alpha1::ExecutionAttemptEnded>(),
        SessionEventCase::CheckpointProduced(_) => event_type::<v1alpha1::CheckpointProduced>(),
        SessionEventCase::DelegationDispatched(_) => event_type::<v1alpha1::DelegationDispatched>(),
        SessionEventCase::ParentLinked(_) => event_type::<v1alpha1::ParentLinked>(),
        SessionEventCase::ParentTerminated(_) => event_type::<v1alpha1::ParentTerminated>(),
        SessionEventCase::DelegationDetached(_) => event_type::<v1alpha1::DelegationDetached>(),
        SessionEventCase::ParentHistoryInvalidated(_) => event_type::<v1alpha1::ParentHistoryInvalidated>(),
        SessionEventCase::ParentDetached(_) => event_type::<v1alpha1::ParentDetached>(),
        SessionEventCase::ExternalDelegationDispatched(_) => event_type::<v1alpha1::ExternalDelegationDispatched>(),
        SessionEventCase::OperationReserved(_) => event_type::<v1alpha1::OperationReserved>(),
        SessionEventCase::OperationOutcomeRecorded(_) => event_type::<v1alpha1::OperationOutcomeRecorded>(),
        SessionEventCase::OperationCancellationRequested(_) => event_type::<v1alpha1::OperationCancellationRequested>(),
        SessionEventCase::ArtifactErased(_) => event_type::<v1alpha1::ArtifactErased>(),
        SessionEventCase::RedactionApplied(_) => event_type::<v1alpha1::RedactionApplied>(),
        SessionEventCase::SystemNoticeRecorded(_) => event_type::<v1alpha1::SystemNoticeRecorded>(),
        SessionEventCase::TodoUpdated(_) => event_type::<v1alpha1::TodoUpdated>(),
        SessionEventCase::SessionRenamed(_) => event_type::<v1alpha1::SessionRenamed>(),
        SessionEventCase::SessionArchived(_) => event_type::<v1alpha1::SessionArchived>(),
        SessionEventCase::SessionUnarchived(_) => event_type::<v1alpha1::SessionUnarchived>(),
        SessionEventCase::ProviderToolIntentRejected(_) => event_type::<v1alpha1::ProviderToolIntentRejected>(),
    }
}

#[cfg(test)]
mod tests;
