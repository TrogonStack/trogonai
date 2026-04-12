use std::future::IntoFuture;

use async_nats::jetstream::{self, context, context::PublishErrorKind, message::PublishMessage};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};
use uuid::Uuid;

use crate::{
    config::{JobWriteCondition, JobWriteState},
    error::CronError,
    events::{JobEvent, JobEventData},
    nats::{
        EventSubjectPrefix, NATS_BATCH_COMMIT, NATS_BATCH_ID, NATS_BATCH_SEQUENCE,
        StreamSubjectState, decode_recorded_job_event, resolve_event_subject_state,
    },
};

use super::{events_stream, project_event_to_snapshot};

async fn current_subject_state<J>(js: &J, subject: &str) -> Result<Option<JobWriteState>, CronError>
where
    J: JetStreamGetStream<Stream = jetstream::stream::Stream>,
{
    let stream = events_stream::run(js).await?;
    match stream.get_last_raw_message_by_subject(subject).await {
        Ok(message) => {
            let version = message.sequence;
            let event = decode_recorded_job_event(message)?;
            let exists = !matches!(event.data, JobEvent::JobRemoved { .. });
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

pub async fn run<J>(
    js: &J,
    job_id: &str,
    write_condition: JobWriteCondition,
    event: JobEventData,
) -> Result<(), CronError>
where
    J: JetStreamGetKeyValue<Store = jetstream::kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>
        + JetStreamPublishMessage<
            PublishError = context::PublishError,
            AckFuture = context::PublishAckFuture,
        >,
{
    let stream = stream_subject_state(js, job_id).await?;
    write_condition.ensure(job_id, stream.write_state)?;
    let expected_version = stream.write_state.current_version().unwrap_or(0);
    let batch_id = Uuid::new_v4().to_string();
    let payload = serde_json::to_vec(&event)?;
    let publish = PublishMessage::build()
        .payload(payload.into())
        .message_id(&event.event_id)
        .expected_last_subject_sequence(expected_version)
        .header(NATS_BATCH_ID, batch_id.as_str())
        .header(NATS_BATCH_SEQUENCE, "1")
        .header(NATS_BATCH_COMMIT, "1");
    let ack = js
        .publish_message(
            publish.outbound_message(event.subject_with_prefix(stream.prefix.as_str())),
        )
        .await
        .map_err(|source| CronError::event_source("failed to publish job event", source))?;

    match ack.into_future().await {
        Ok(_) => {}
        Err(error) if error.kind() == PublishErrorKind::WrongLastSequence => {
            return Err(CronError::OptimisticConcurrencyConflict {
                id: job_id.to_string(),
                expected: write_condition,
                current_version: stream_subject_state(js, job_id)
                    .await?
                    .write_state
                    .current_version(),
            });
        }
        Err(error) => {
            return Err(CronError::event_source(
                "failed to acknowledge job event batch",
                error,
            ));
        }
    }

    project_event_to_snapshot::run(js, job_id, &event).await?;

    Ok(())
}
