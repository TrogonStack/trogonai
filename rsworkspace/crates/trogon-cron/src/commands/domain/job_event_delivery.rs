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
