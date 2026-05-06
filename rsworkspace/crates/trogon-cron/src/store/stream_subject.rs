use async_nats::jetstream;
use trogon_eventsourcing::nats::jetstream::{StreamSubjectResolver, SubjectState, subject_current_position};

use crate::{
    config::JobWriteState,
    error::CronError,
    nats::{EventSubjectPrefix, StreamSubjectState, resolve_event_subject_state},
};

async fn current_subject_state(
    stream: &jetstream::stream::Stream,
    subject: &str,
) -> Result<Option<JobWriteState>, CronError> {
    subject_current_position(stream, subject)
        .await
        .map_err(|source| CronError::event_source("failed to read latest stream position", source))
        .map(|position| position.map(|position| JobWriteState::new(Some(position), true)))
}

pub(crate) async fn stream_subject_state(
    stream: &jetstream::stream::Stream,
    job_id: &str,
) -> Result<StreamSubjectState, CronError> {
    let canonical_state = current_subject_state(stream, &EventSubjectPrefix::Canonical.subject(job_id)).await?;
    let legacy_state = current_subject_state(stream, &EventSubjectPrefix::Legacy.subject(job_id)).await?;

    resolve_event_subject_state(job_id, canonical_state, legacy_state)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JobEventSubjectResolver;

impl StreamSubjectResolver<str> for JobEventSubjectResolver {
    type Error = CronError;

    async fn resolve_subject_state(
        &self,
        events_stream: &jetstream::stream::Stream,
        stream_id: &str,
    ) -> Result<SubjectState, Self::Error> {
        let state = stream_subject_state(events_stream, stream_id).await?;
        Ok(SubjectState {
            subject: state.prefix.subject(stream_id),
            current_position: state.write_state.current_position(),
        })
    }
}
