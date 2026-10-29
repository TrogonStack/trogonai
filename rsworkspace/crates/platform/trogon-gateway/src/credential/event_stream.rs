use async_nats::jetstream::stream::{InfoError, RawMessageError, RawMessageErrorKind};
use trogon_decider_nats::StreamStoreError;
use trogon_decider_runtime::StreamEvent;
use trogon_nats::jetstream::{JetStreamGetRawMessage, JetStreamGetStreamInfo};

#[derive(Debug, thiserror::Error)]
pub enum CredentialEventStreamReadError {
    #[error("failed to query stream info: {source}")]
    QueryStreamInfo {
        #[source]
        source: InfoError,
    },
    #[error("failed to read stream message: {source}")]
    ReadStreamMessage {
        #[source]
        source: RawMessageError,
    },
    #[error(transparent)]
    RecordStreamMessage(StreamStoreError),
}

/// Reads all credential events from the JetStream stream starting at a stream
/// sequence.
///
/// Sequence-by-sequence raw reads keep this generic over the JetStream access
/// traits so processors stay testable with mock streams; deleted sequences are
/// skipped.
pub async fn read_credential_event_stream<S>(
    stream: &S,
    from_sequence: u64,
) -> Result<Vec<StreamEvent>, CredentialEventStreamReadError>
where
    S: JetStreamGetStreamInfo + JetStreamGetRawMessage,
{
    let info = stream
        .get_info()
        .await
        .map_err(|source| CredentialEventStreamReadError::QueryStreamInfo { source })?;
    let to_sequence = info.state.last_sequence;
    if from_sequence == 0 || to_sequence == 0 || from_sequence > to_sequence {
        return Ok(Vec::new());
    }

    let mut events = Vec::new();
    for sequence in from_sequence..=to_sequence {
        let message = match stream.get_raw_message(sequence).await {
            Ok(message) => message,
            Err(source) if matches!(source.kind(), RawMessageErrorKind::NoMessageFound) => continue,
            Err(source) => return Err(CredentialEventStreamReadError::ReadStreamMessage { source }),
        };
        let stream_id = message.subject.to_string();
        let event = trogon_decider_nats::record_stream_message(message, stream_id)
            .map_err(CredentialEventStreamReadError::RecordStreamMessage)?;
        events.push(event);
    }
    Ok(events)
}
