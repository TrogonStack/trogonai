use async_nats::jetstream;

use super::{JetStreamStore, JetStreamStoreError, StreamSubjectResolver};
use crate::nats::stream_store::{StreamStoreError, append_stream as append_subject_stream, read_subject_stream};
use crate::{
    AppendStreamRequest, AppendStreamResponse, ReadStreamRequest, ReadStreamResponse, StreamAppend, StreamRead,
    StreamState,
};

impl<StreamId, Resolver> StreamRead<StreamId> for JetStreamStore<Resolver>
where
    StreamId: AsRef<str> + ToString + Send + Sync + ?Sized,
    Resolver: StreamSubjectResolver<StreamId>,
{
    type Error = JetStreamStoreError<Resolver::Error>;

    async fn read_stream(&self, request: ReadStreamRequest<'_, StreamId>) -> Result<ReadStreamResponse, Self::Error> {
        let stream_id = request.stream_id;
        let subject_state = self
            .subject_resolver
            .resolve_subject_state(self.events_stream(), stream_id)
            .await
            .map_err(JetStreamStoreError::ResolveSubject)?;
        let current_version = subject_state.current_version;
        let events = read_subject_stream(self.events_stream(), &subject_state.subject, request.from_sequence)
            .await
            .map_err(JetStreamStoreError::ReadStream)?
            .into_iter()
            .map(|mut event| {
                event.event_stream_id = stream_id.as_ref().to_string();
                event
            })
            .collect();

        Ok(ReadStreamResponse {
            current_version,
            events,
        })
    }
}

impl<StreamId, Resolver> StreamAppend<StreamId> for JetStreamStore<Resolver>
where
    StreamId: AsRef<str> + ToString + Send + Sync + ?Sized,
    Resolver: StreamSubjectResolver<StreamId>,
{
    type Error = JetStreamStoreError<Resolver::Error>;

    async fn append_stream(
        &self,
        request: AppendStreamRequest<'_, StreamId>,
    ) -> Result<AppendStreamResponse, Self::Error> {
        let stream_id = request.stream_id;
        let expected_state = request.stream_state;
        let events = request.events;
        let subject_state = self
            .subject_resolver
            .resolve_subject_state(self.events_stream(), stream_id)
            .await
            .map_err(JetStreamStoreError::ResolveSubject)?;
        if events.iter().any(|event| event.stream_id() != stream_id.as_ref()) {
            return Err(JetStreamStoreError::AppendStream(StreamStoreError::publish_source(
                "failed to publish stream event batch",
                std::io::Error::other(format!("batch contains events outside stream '{}'", stream_id.as_ref())),
            )));
        }
        let current_version = subject_state.current_version;
        let expected_last_subject_sequence =
            resolve_expected_last_subject_sequence(stream_id, expected_state, current_version)?;

        let next_expected_version = append_subject_stream(
            self.as_jetstream(),
            subject_state.subject,
            expected_last_subject_sequence,
            &events,
        )
        .await
        .map_err(|source| match source {
            StreamStoreError::WrongExpectedVersion => JetStreamStoreError::OptimisticConcurrencyConflict {
                stream_id: stream_id.to_string(),
                expected: expected_state,
                current_version,
            },
            other => JetStreamStoreError::AppendStream(other),
        })?;

        Ok(AppendStreamResponse { next_expected_version })
    }
}

fn resolve_expected_last_subject_sequence<StreamId, Error>(
    stream_id: &StreamId,
    expected_state: StreamState,
    current_version: Option<u64>,
) -> Result<Option<u64>, JetStreamStoreError<Error>>
where
    StreamId: ToString + ?Sized,
{
    match expected_state {
        StreamState::Any => Ok(None),
        StreamState::StreamExists => {
            current_version
                .map(|_| None)
                .ok_or_else(|| JetStreamStoreError::OptimisticConcurrencyConflict {
                    stream_id: stream_id.to_string(),
                    expected: StreamState::StreamExists,
                    current_version,
                })
        }
        StreamState::NoStream => Ok(Some(0)),
        StreamState::StreamRevision(version) => Ok(Some(version)),
    }
}

pub async fn subject_current_version(
    stream: &jetstream::stream::Stream,
    subject: &str,
) -> Result<Option<u64>, StreamStoreError> {
    match stream.get_last_raw_message_by_subject(subject).await {
        Ok(message) => Ok(Some(message.sequence)),
        Err(error)
            if matches!(
                error.kind(),
                async_nats::jetstream::stream::LastRawMessageErrorKind::NoMessageFound
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(StreamStoreError::Read {
            context: "failed to read latest subject message",
            source: Box::new(error),
        }),
    }
}
