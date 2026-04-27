use serde::{Deserialize, Serialize};
use trogon_cron_jobs_proto::v1;

use crate::commands::proto::JobEventProtoError;

use super::JobEventSamplingSource;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobEventDelivery {
    NatsEvent {
        route: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ttl_sec: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<JobEventSamplingSource>,
    },
}

impl From<&JobEventDelivery> for v1::JobDelivery {
    fn from(value: &JobEventDelivery) -> Self {
        let mut delivery = v1::JobDelivery::new();
        match value {
            JobEventDelivery::NatsEvent { route, ttl_sec, source } => {
                let mut inner = v1::NatsEventDelivery::new();
                inner.set_route(route.as_str());
                if let Some(ttl_sec) = ttl_sec {
                    inner.set_ttl_sec(*ttl_sec);
                }
                if let Some(source) = source {
                    inner.set_source(v1::JobSamplingSource::from(source));
                }
                delivery.set_nats_event(inner);
            }
        }
        delivery
    }
}

impl TryFrom<v1::JobDelivery> for JobEventDelivery {
    type Error = JobEventProtoError;

    fn try_from(value: v1::JobDelivery) -> Result<Self, Self::Error> {
        match value.kind() {
            v1::job_delivery::KindOneof::NatsEvent(inner) => Ok(Self::NatsEvent {
                route: inner.route().to_string(),
                ttl_sec: inner.has_ttl_sec().then(|| inner.ttl_sec()),
                source: inner
                    .has_source()
                    .then(|| inner.source().to_owned().try_into())
                    .transpose()?,
            }),
            v1::job_delivery::KindOneof::not_set(_) | _ => Err(JobEventProtoError::MissingDeliveryKind),
        }
    }
}
