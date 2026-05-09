use buffa::MessageField;
use trogon_cron_jobs_proto::v1;

use super::JobEventSamplingSource;

#[derive(Debug, Clone, PartialEq)]
pub enum JobEventDelivery {
    NatsEvent {
        route: String,
        ttl_sec: Option<u64>,
        source: Option<JobEventSamplingSource>,
    },
}

impl From<&JobEventDelivery> for v1::JobDelivery {
    fn from(value: &JobEventDelivery) -> Self {
        match value {
            JobEventDelivery::NatsEvent { route, ttl_sec, source } => v1::JobDelivery {
                kind: Some(
                    v1::NatsEventDelivery {
                        route: route.clone(),
                        ttl_sec: *ttl_sec,
                        source: source
                            .as_ref()
                            .map(v1::JobSamplingSource::from)
                            .map(MessageField::some)
                            .unwrap_or_else(MessageField::none),
                    }
                    .into(),
                ),
            },
        }
    }
}
