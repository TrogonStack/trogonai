use serde::{Deserialize, Serialize};
use trogon_cron_jobs_proto::v1;

use crate::proto::JobEventProtoError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobEventSamplingSource {
    LatestFromSubject { subject: String },
}

impl From<&JobEventSamplingSource> for v1::JobSamplingSource {
    fn from(value: &JobEventSamplingSource) -> Self {
        let mut source = v1::JobSamplingSource::new();
        match value {
            JobEventSamplingSource::LatestFromSubject { subject } => {
                let mut inner = v1::LatestFromSubjectSampling::new();
                inner.set_subject(subject.as_str());
                source.set_latest_from_subject(inner);
            }
        }
        source
    }
}

impl TryFrom<v1::JobSamplingSource> for JobEventSamplingSource {
    type Error = JobEventProtoError;

    fn try_from(value: v1::JobSamplingSource) -> Result<Self, Self::Error> {
        match value.kind() {
            v1::job_sampling_source::KindOneof::LatestFromSubject(inner) => Ok(Self::LatestFromSubject {
                subject: inner.subject().to_string(),
            }),
            v1::job_sampling_source::KindOneof::not_set(_) | _ => Err(JobEventProtoError::MissingSamplingSourceKind),
        }
    }
}
