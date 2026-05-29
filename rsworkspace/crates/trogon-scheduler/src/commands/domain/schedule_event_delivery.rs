use buffa::MessageField;
use buffa_types::google::protobuf::Duration;
use trogonai_proto::scheduler::schedules::v1;

use super::ScheduleEventSamplingSource;

#[derive(Debug, Clone, PartialEq)]
pub enum ScheduleEventDelivery {
    NatsMessage {
        subject: String,
        ttl_sec: Option<u64>,
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
                        subject: subject.clone(),
                        ttl: ttl_sec
                            .map(|secs| {
                                MessageField::some(Duration {
                                    seconds: secs as i64,
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
