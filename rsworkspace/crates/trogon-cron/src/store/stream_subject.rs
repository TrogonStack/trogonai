use async_nats::jetstream;
use trogon_decider_nats::{StreamSubject, StreamSubjectResolver, SubjectState, subject_current_position};

use crate::{
    config::JobWriteState,
    error::CronError,
    nats::{event_subject, resolve_event_subject_state},
};

async fn current_subject_state(
    stream: &jetstream::stream::Stream,
    subject: &StreamSubject,
) -> Result<Option<JobWriteState>, CronError> {
    subject_current_position(stream, subject)
        .await
        .map_err(|source| CronError::event_source("failed to read latest stream position", source))
        .map(|position| position.map(|position| JobWriteState::new(Some(position), true)))
}

fn job_event_subject(job_id: &str) -> Result<StreamSubject, CronError> {
    StreamSubject::new(event_subject(job_id))
        .map_err(|source| CronError::event_source("failed to resolve job event subject", source))
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
        let subject = job_event_subject(stream_id)?;
        let canonical_state = current_subject_state(events_stream, &subject).await?;
        let state = resolve_event_subject_state(canonical_state);
        Ok(SubjectState {
            subject,
            current_position: state.write_state.current_position(),
        })
    }
}
