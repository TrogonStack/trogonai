use async_nats::jetstream;
use trogon_eventsourcing::jetstream::{
    StreamSubjectResolver, SubjectState, subject_current_version,
};

use crate::{
    JobId,
    config::JobWriteState,
    error::CronError,
    nats::{EventSubjectPrefix, StreamSubjectState, resolve_event_subject_state},
};

async fn current_subject_state(
    stream: &jetstream::stream::Stream,
    subject: &str,
) -> Result<Option<JobWriteState>, CronError> {
    subject_current_version(stream, subject)
        .await
        .map_err(|source| CronError::event_source("failed to read latest stream version", source))
        .map(|version| version.map(|version| JobWriteState::new(Some(version), true)))
}

pub(crate) async fn stream_subject_state(
    stream: &jetstream::stream::Stream,
    job_id: &str,
) -> Result<StreamSubjectState, CronError> {
    let canonical_state =
        current_subject_state(stream, &EventSubjectPrefix::Canonical.subject(job_id)).await?;
    let legacy_state =
        current_subject_state(stream, &EventSubjectPrefix::Legacy.subject(job_id)).await?;

    resolve_event_subject_state(job_id, canonical_state, legacy_state)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct JobEventSubjectResolver;

impl StreamSubjectResolver<JobId> for JobEventSubjectResolver {
    type Error = CronError;

    async fn resolve_subject_state(
        &self,
        events_stream: &jetstream::stream::Stream,
        stream_id: &JobId,
    ) -> Result<SubjectState, Self::Error> {
        let state = stream_subject_state(events_stream, stream_id.as_str()).await?;
        Ok(SubjectState {
            subject: state.prefix.subject(stream_id.as_str()),
            current_version: state.write_state.current_version(),
        })
    }
}
