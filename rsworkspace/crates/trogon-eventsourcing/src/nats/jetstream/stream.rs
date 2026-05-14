use async_nats::jetstream;

use super::{JetStreamStore, JetStreamStoreError, StreamSubjectResolver};
use crate::nats::stream_store::{StreamStoreError, append_stream as append_subject_stream, read_subject_stream};
use crate::{
    AppendStreamRequest, AppendStreamResponse, ReadStreamRequest, ReadStreamResponse, StreamAppend, StreamPosition,
    StreamRead, StreamState,
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
        let current_position = subject_state.current_position;
        let events = read_subject_stream(self.events_stream(), &subject_state.subject, request.from_sequence)
            .await
            .map_err(JetStreamStoreError::ReadStream)?
            .into_iter()
            .map(|mut event| {
                event.stream_id = stream_id.as_ref().to_string();
                event
            })
            .collect();

        Ok(ReadStreamResponse {
            current_position,
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
        let current_position = subject_state.current_position;
        let expected_last_subject_sequence =
            resolve_expected_last_subject_sequence(stream_id, expected_state, current_position)?;

        let stream_position = append_subject_stream(
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
                current_position,
            },
            other => JetStreamStoreError::AppendStream(other),
        })?;

        Ok(AppendStreamResponse { stream_position })
    }
}

fn resolve_expected_last_subject_sequence<StreamId, Error>(
    stream_id: &StreamId,
    expected_state: StreamState,
    current_position: Option<StreamPosition>,
) -> Result<Option<u64>, JetStreamStoreError<Error>>
where
    StreamId: ToString + ?Sized,
{
    match expected_state {
        StreamState::Any => Ok(None),
        StreamState::StreamExists => {
            current_position
                .map(|_| None)
                .ok_or_else(|| JetStreamStoreError::OptimisticConcurrencyConflict {
                    stream_id: stream_id.to_string(),
                    expected: StreamState::StreamExists,
                    current_position,
                })
        }
        StreamState::NoStream => Ok(Some(0)),
        StreamState::At(position) => Ok(Some(position.get())),
    }
}

pub async fn subject_current_position(
    stream: &jetstream::stream::Stream,
    subject: &str,
) -> Result<Option<StreamPosition>, StreamStoreError> {
    match stream.get_last_raw_message_by_subject(subject).await {
        Ok(message) => StreamPosition::try_new(message.sequence)
            .map(Some)
            .map_err(|source| StreamStoreError::read_source("failed to read latest subject position", source)),
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
