use buffa::MessageField;
use trogonai_proto::convert::{DurationConversionError, duration_from_std};
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

impl TryFrom<&ScheduleEventDelivery> for v1::Delivery {
    type Error = DurationConversionError;

    fn try_from(value: &ScheduleEventDelivery) -> Result<Self, Self::Error> {
        match value {
            ScheduleEventDelivery::NatsMessage { subject, ttl, source } => Ok(v1::Delivery {
                kind: Some(
                    v1::delivery::NatsMessage {
                        subject: subject.as_str().to_string(),
                        ttl: match ttl {
                            Some(ttl) => MessageField::some(duration_from_std(ttl.as_duration())?),
                            None => MessageField::none(),
                        },
                        source: source
                            .as_ref()
                            .map(v1::delivery::nats_message::Source::from)
                            .map(MessageField::some)
                            .unwrap_or_else(MessageField::none),
                    }
                    .into(),
                ),
            }),
        }
    }
}
