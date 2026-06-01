use buffa::MessageField;
use trogonai_proto::convert::duration_from_std;
use trogonai_proto::scheduler::schedules::v1;

use super::{DeliveryRoute, ScheduleEventSamplingSource, TtlDuration};

#[derive(Debug, Clone, PartialEq)]
pub enum ScheduleEventDelivery {
    NatsMessage {
        subject: DeliveryRoute,
        ttl: Option<TtlDuration>,
        source: Option<ScheduleEventSamplingSource>,
    },
}

impl From<&ScheduleEventDelivery> for v1::Delivery {
    fn from(value: &ScheduleEventDelivery) -> Self {
        match value {
            ScheduleEventDelivery::NatsMessage { subject, ttl, source } => v1::Delivery {
                kind: Some(
                    v1::delivery::NatsMessage {
                        subject: subject.as_str().to_string(),
                        ttl: ttl
                            .map(|ttl| MessageField::some(duration_from_std(ttl.as_duration())))
                            .unwrap_or_else(MessageField::none),
                        source: source
                            .as_ref()
                            .map(v1::delivery::nats_message::Source::from)
                            .map(MessageField::some)
                            .unwrap_or_else(MessageField::none),
                    }
                    .into(),
                ),
            },
        }
    }
}
