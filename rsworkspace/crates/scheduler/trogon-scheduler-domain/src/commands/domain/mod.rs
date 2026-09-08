#![cfg_attr(
    dylint_lib = "trogon_lints",
    expect(
        acyclic_modules,
        reason = "a schedule is defined by the delivery and event value objects it holds and each of those is typed with the schedule vocabulary it belongs to"
    )
)]

mod message;
mod schedule;
mod schedule_event_delivery;
mod schedule_event_sampling_source;
mod schedule_event_schedule;
mod schedule_event_status;
mod schedule_id;

pub use message::{
    HeaderName, HeaderValue, MessageContent, MessageContentType, MessageEnvelope, MessageHeader, MessageHeaders,
    MessageHeadersError,
};
pub use schedule::{
    CronExpression, CronExpressionError, Delivery, DeliveryRoute, DeliveryRouteError, EveryDuration,
    EveryDurationError, RRuleDateTime, RRuleDateTimeError, RRuleExpression, RRuleExpressionError, RRuleTimezone,
    SamplingSource, SamplingSubject, SamplingSubjectError, Schedule, ScheduleError, ScheduleHeaders,
    ScheduleHeadersError, ScheduleMessage, ScheduleTimezone, TimeZone, TimeZoneError, TtlDuration, TtlDurationError,
    TzdbVersion, TzdbVersionError,
};
pub use schedule_event_delivery::ScheduleEventDelivery;
pub use schedule_event_sampling_source::ScheduleEventSamplingSource;
pub use schedule_event_schedule::ScheduleEventSchedule;
pub use schedule_event_status::ScheduleEventStatus;
pub use schedule_id::{ScheduleId, ScheduleIdError};
