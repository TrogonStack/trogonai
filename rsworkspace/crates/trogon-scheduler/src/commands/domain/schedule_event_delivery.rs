use buffa::MessageField;
use buffa_types::google::protobuf::Duration;
use trogonai_proto::scheduler::schedules::v1;

use super::{DeliveryRoute, ScheduleEventSamplingSource, TtlSeconds};

#[derive(Debug, Clone, PartialEq)]
pub enum ScheduleEventDelivery {
    NatsMessage {
        subject: DeliveryRoute,
        ttl_sec: Option<TtlSeconds>,
        source: Option<ScheduleEventSamplingSource>,
    },
}

impl From<&ScheduleEventDelivery> for v1::Delivery {
    fn from(value: &ScheduleEventDelivery) -> Self {
        match value {
            ScheduleEventDelivery::NatsMessage {
                subject,
                ttl_sec,
                source,
            } => v1::Delivery {
                kind: Some(
                    v1::delivery::NatsMessage {
                        subject: subject.as_str().to_string(),
                        ttl: ttl_sec
                            .map(|secs| {
                                MessageField::some(Duration {
                                    seconds: i64::try_from(secs.as_u64()).unwrap_or(i64::MAX),
                                    nanos: 0,
                                    ..Duration::default()
                                })
                            })
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
