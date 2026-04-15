#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use async_nats::jetstream::{self, context};
use chrono::{DateTime, Utc};
use trogon_eventsourcing::{NonEmpty, StreamStoreError, append_stream};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

use crate::{
    config::{JobWriteCondition, JobWriteState},
    error::CronError,
    events::{JobEvent, JobEventData},
    nats::{EventSubjectPrefix, StreamSubjectState, resolve_event_subject_state},
    projections::project_appended_events,
};

use super::open_events_stream;

fn decode_job_event_data(payload: &[u8]) -> Result<JobEventData, CronError> {
    JobEventData::decode(payload)
        .map_err(|source| CronError::event_source("failed to decode stored job event", source))
}

fn decode_recorded_job_event(
    message: async_nats::jetstream::message::StreamMessage,
) -> Result<crate::RecordedJobEvent, CronError> {
    let recorded_at = recorded_at_from_message(&message)?;
    let stream_id = message.subject.to_string();
    let log_position = Some(message.sequence);
    let event = decode_job_event_data(&message.payload)?;

    Ok(event.record(stream_id, None, log_position, recorded_at))
}

fn recorded_at_from_message(
    message: &async_nats::jetstream::message::StreamMessage,
) -> Result<DateTime<Utc>, CronError> {
    DateTime::<Utc>::from_timestamp(message.time.unix_timestamp(), message.time.nanosecond())
        .ok_or_else(|| {
            CronError::event_source(
                "failed to convert message timestamp into recorded event time",
                std::io::Error::other(message.subject.to_string()),
            )
        })
}

#[cfg(not(coverage))]
async fn current_subject_state<J>(js: &J, subject: &str) -> Result<Option<JobWriteState>, CronError>
where
    J: JetStreamGetStream<Stream = jetstream::stream::Stream>,
{
    let stream = open_events_stream(js).await?;
    match stream.get_last_raw_message_by_subject(subject).await {
        Ok(message) => {
            let version = message.sequence;
            let event = decode_recorded_job_event(message)?;
            let event = event.decode_data::<JobEvent>().map_err(|source| {
                CronError::event_source("failed to decode latest job event payload", source)
            })?;
            let exists = !matches!(event, JobEvent::JobRemoved { .. });
            Ok(Some(JobWriteState::new(Some(version), exists)))
        }
        Err(error)
            if matches!(
                error.kind(),
                async_nats::jetstream::stream::LastRawMessageErrorKind::NoMessageFound
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(CronError::event_source(
            "failed to read latest stream version",
            error,
        )),
    }
}

#[cfg(coverage)]
async fn current_subject_state<J>(
    _js: &J,
    _subject: &str,
) -> Result<Option<JobWriteState>, CronError>
where
    J: JetStreamGetStream<Stream = jetstream::stream::Stream>,
{
    Ok(None)
}

#[cfg(not(coverage))]
pub async fn stream_subject_state<J>(js: &J, job_id: &str) -> Result<StreamSubjectState, CronError>
where
    J: JetStreamGetStream<Stream = jetstream::stream::Stream>,
{
    let canonical_version =
        current_subject_state(js, &EventSubjectPrefix::Canonical.subject(job_id)).await?;
    let legacy_version =
        current_subject_state(js, &EventSubjectPrefix::Legacy.subject(job_id)).await?;

    resolve_event_subject_state(job_id, canonical_version, legacy_version)
}

#[cfg(coverage)]
pub async fn stream_subject_state<J>(_js: &J, job_id: &str) -> Result<StreamSubjectState, CronError>
where
    J: JetStreamGetStream<Stream = jetstream::stream::Stream>,
{
    let _ = job_id;
    Ok(StreamSubjectState {
        prefix: EventSubjectPrefix::Canonical,
        write_state: JobWriteState::new(None, false),
    })
}

#[cfg(not(coverage))]
pub async fn run<J>(
    js: &J,
    job_id: &str,
    write_condition: JobWriteCondition,
    events: NonEmpty<JobEventData>,
) -> Result<(), CronError>
where
    J: JetStreamGetKeyValue<Store = jetstream::kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>
        + JetStreamPublishMessage<
            PublishError = context::PublishError,
            AckFuture = context::PublishAckFuture,
        >,
{
    if events.iter().any(|event| event.stream_id() != job_id) {
        return Err(CronError::event_source(
            "failed to publish job event batch",
            std::io::Error::other(format!("batch contains events outside stream '{job_id}'")),
        ));
    }

    let stream = stream_subject_state(js, job_id).await?;
    write_condition.ensure(job_id, stream.write_state)?;
    let expected_version = stream.write_state.current_version().unwrap_or(0);
    let subject = events.first().subject_with_prefix(stream.prefix.as_str());
    match append_stream(js, subject, expected_version, &events).await {
        Ok(()) => {}
        Err(StreamStoreError::WrongExpectedVersion) => {
            return Err(CronError::OptimisticConcurrencyConflict {
                id: job_id.to_string(),
                expected: write_condition.into(),
                current_version: stream_subject_state(js, job_id)
                    .await?
                    .write_state
                    .current_version(),
            });
        }
        Err(source) => {
            return Err(CronError::event_source(
                "failed to append job event batch",
                source,
            ));
        }
    }

    project_appended_events(js, job_id, events.as_slice()).await?;

    Ok(())
}

#[cfg(coverage)]
pub async fn run<J>(
    _js: &J,
    _job_id: &str,
    _write_condition: JobWriteCondition,
    _events: NonEmpty<JobEventData>,
) -> Result<(), CronError>
where
    J: JetStreamGetKeyValue<Store = jetstream::kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>
        + JetStreamPublishMessage<
            PublishError = context::PublishError,
            AckFuture = context::PublishAckFuture,
        >,
{
    Ok(())
}
