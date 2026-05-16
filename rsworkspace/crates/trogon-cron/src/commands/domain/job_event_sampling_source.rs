use trogon_cron_jobs_proto::v1;

#[derive(Debug, Clone, PartialEq)]
pub enum JobEventSamplingSource {
    LatestFromSubject { subject: String },
}

impl From<&JobEventSamplingSource> for v1::JobSamplingSource {
    fn from(value: &JobEventSamplingSource) -> Self {
        match value {
            JobEventSamplingSource::LatestFromSubject { subject } => v1::JobSamplingSource {
                kind: Some(
                    v1::LatestFromSubjectSampling {
                        subject: subject.clone(),
                    }
                    .into(),
                ),
            },
        }
    }
}
