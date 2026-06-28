//! Decode scheduler command protos into domain command types.

use trogonai_proto::content::v1alpha1 as content_v1alpha1;
use trogonai_proto::convert::{TimestampConversionError, datetime_from_timestamp, std_from_duration};
use trogonai_proto::scheduler::schedules::v1;

use super::domain::{
    Delivery, DeliveryRoute, EveryDuration, MessageContent, MessageHeaders, RRuleDateTime, RRuleExpression,
    RRuleTimezone, SamplingSource, SamplingSubject, Schedule, ScheduleEventStatus, ScheduleHeaders, ScheduleId,
    ScheduleMessage, TtlDuration,
};
use super::{CreateSchedule, PauseSchedule, RemoveSchedule, ResumeSchedule};

#[derive(Debug, thiserror::Error)]
pub enum CommandWireError {
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("invalid schedule id: {0}")]
    InvalidScheduleId(#[from] super::domain::ScheduleIdError),
    #[error("invalid schedule status")]
    InvalidStatus,
    #[error("invalid schedule definition: {0}")]
    InvalidSchedule(#[from] super::domain::ScheduleError),
    #[error("invalid delivery route: {0}")]
    InvalidDeliveryRoute(#[from] super::domain::DeliveryRouteError),
    #[error("invalid ttl duration: {0}")]
    InvalidTtl(#[from] super::domain::TtlDurationError),
    #[error("invalid every duration: {0}")]
    InvalidEvery(#[from] super::domain::EveryDurationError),
    #[error("invalid message headers: {0}")]
    InvalidHeaders(#[from] super::domain::MessageHeadersError),
    #[error("invalid schedule headers: {0}")]
    InvalidScheduleHeaders(#[from] super::domain::ScheduleHeadersError),
    #[error("invalid rrule datetime: {0}")]
    InvalidRRuleDateTime(#[from] super::domain::RRuleDateTimeError),
    #[error("invalid rrule expression: {0}")]
    InvalidRRuleExpression(#[from] super::domain::RRuleExpressionError),
    #[error("invalid timezone: {0}")]
    InvalidTimezone(#[from] super::domain::TimeZoneError),
    #[error("invalid sampling subject: {0}")]
    InvalidSamplingSubject(#[from] super::domain::SamplingSubjectError),
    #[error("invalid timestamp: {0}")]
    InvalidTimestamp(#[from] TimestampConversionError),
    #[error("invalid duration")]
    InvalidDuration(#[from] trogonai_proto::convert::StdDurationConversionError),
}

impl TryFrom<v1::CreateSchedule> for CreateSchedule {
    type Error = CommandWireError;

    fn try_from(value: v1::CreateSchedule) -> Result<Self, Self::Error> {
        if value.schedule_id.is_empty() {
            return Err(CommandWireError::MissingField("schedule_id"));
        }

        Ok(Self {
            id: ScheduleId::parse(&value.schedule_id)?,
            status: status_from_proto(
                value
                    .status
                    .as_option()
                    .ok_or(CommandWireError::MissingField("status"))?,
            )?,
            schedule: schedule_from_proto(
                value
                    .schedule
                    .as_option()
                    .ok_or(CommandWireError::MissingField("schedule"))?,
            )?,
            delivery: delivery_from_proto(
                value
                    .delivery
                    .as_option()
                    .ok_or(CommandWireError::MissingField("delivery"))?,
            )?,
            message: message_from_proto(
                value
                    .message
                    .as_option()
                    .ok_or(CommandWireError::MissingField("message"))?,
            )?,
        })
    }
}

impl TryFrom<v1::PauseSchedule> for PauseSchedule {
    type Error = CommandWireError;

    fn try_from(value: v1::PauseSchedule) -> Result<Self, Self::Error> {
        if value.schedule_id.is_empty() {
            return Err(CommandWireError::MissingField("schedule_id"));
        }
        Ok(PauseSchedule::new(ScheduleId::parse(&value.schedule_id)?))
    }
}

impl TryFrom<v1::RemoveSchedule> for RemoveSchedule {
    type Error = CommandWireError;

    fn try_from(value: v1::RemoveSchedule) -> Result<Self, Self::Error> {
        if value.schedule_id.is_empty() {
            return Err(CommandWireError::MissingField("schedule_id"));
        }
        Ok(RemoveSchedule::new(ScheduleId::parse(&value.schedule_id)?))
    }
}

impl TryFrom<v1::ResumeSchedule> for ResumeSchedule {
    type Error = CommandWireError;

    fn try_from(value: v1::ResumeSchedule) -> Result<Self, Self::Error> {
        if value.schedule_id.is_empty() {
            return Err(CommandWireError::MissingField("schedule_id"));
        }
        Ok(ResumeSchedule::new(ScheduleId::parse(&value.schedule_id)?))
    }
}

fn status_from_proto(status: &v1::ScheduleStatus) -> Result<ScheduleEventStatus, CommandWireError> {
    let Some(kind) = status.kind.as_ref() else {
        return Err(CommandWireError::InvalidStatus);
    };
    Ok(match kind {
        v1::schedule_status::Kind::Scheduled(_) => ScheduleEventStatus::Scheduled,
        v1::schedule_status::Kind::Paused(_) => ScheduleEventStatus::Paused,
    })
}

fn schedule_from_proto(schedule: &v1::Schedule) -> Result<Schedule, CommandWireError> {
    let Some(kind) = schedule.kind.as_ref() else {
        return Err(CommandWireError::MissingField("schedule.kind"));
    };

    match kind {
        v1::schedule::Kind::At(at) => {
            let timestamp = at.at.as_option().ok_or(CommandWireError::MissingField("schedule.at"))?;
            Ok(Schedule::At {
                at: datetime_from_timestamp(timestamp)?,
            })
        }
        v1::schedule::Kind::Every(every) => {
            let duration = every
                .every
                .as_option()
                .ok_or(CommandWireError::MissingField("schedule.every"))?;
            Ok(Schedule::Every {
                every: EveryDuration::try_from(std_from_duration(duration)?)?,
            })
        }
        v1::schedule::Kind::Cron(cron) => {
            Schedule::cron(cron.expr.clone(), timezone_id_from_proto(cron.timezone.as_option()))
                .map_err(CommandWireError::from)
        }
        v1::schedule::Kind::Rrule(rrule) => {
            let dtstart = rrule
                .dtstart
                .as_option()
                .ok_or(CommandWireError::MissingField("schedule.rrule.dtstart"))?;
            let dtstart = datetime_from_timestamp(dtstart)?;
            Ok(Schedule::RRule {
                dtstart: RRuleDateTime::new("dtstart", dtstart.to_rfc3339())?,
                rrule: RRuleExpression::new(rrule.rrule.clone())?,
                timezone: timezone_id_from_proto(rrule.timezone.as_option())
                    .map(RRuleTimezone::new)
                    .transpose()?,
                rdate: rrule
                    .rdate
                    .iter()
                    .map(|timestamp| -> Result<_, CommandWireError> {
                        let value = datetime_from_timestamp(timestamp)?;
                        Ok(RRuleDateTime::new("rdate", value.to_rfc3339())?)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                exdate: rrule
                    .exdate
                    .iter()
                    .map(|timestamp| -> Result<_, CommandWireError> {
                        let value = datetime_from_timestamp(timestamp)?;
                        Ok(RRuleDateTime::new("exdate", value.to_rfc3339())?)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            })
        }
    }
}

fn timezone_id_from_proto(timezone: Option<&trogonai_proto::google::r#type::TimeZone>) -> Option<String> {
    timezone.map(|value| value.id.clone()).filter(|id| !id.is_empty())
}

fn delivery_from_proto(delivery: &v1::Delivery) -> Result<Delivery, CommandWireError> {
    let Some(v1::delivery::Kind::NatsMessage(message)) = delivery.kind.as_ref() else {
        return Err(CommandWireError::MissingField("delivery.nats_message"));
    };

    let ttl = match message.ttl.as_option() {
        Some(duration) => Some(TtlDuration::try_from(std_from_duration(duration)?)?),
        None => None,
    };
    let source = match message.source.as_option() {
        Some(source) => match source.kind.as_ref() {
            Some(v1::delivery::nats_message::source::Kind::LatestFromSubject(inner)) => {
                Some(SamplingSource::LatestFromSubject {
                    subject: SamplingSubject::new(&inner.subject)?,
                })
            }
            None => return Err(CommandWireError::MissingField("delivery.source.kind")),
        },
        None => None,
    };

    Ok(Delivery::NatsEvent {
        route: DeliveryRoute::new(&message.subject)?,
        ttl,
        source,
    })
}

fn message_from_proto(message: &v1::Message) -> Result<ScheduleMessage, CommandWireError> {
    let content = message
        .content
        .as_option()
        .ok_or(CommandWireError::MissingField("message.content"))?;
    let headers = MessageHeaders::new(
        message
            .headers
            .iter()
            .map(|header| (header.name.as_str(), header.value.as_str())),
    )?;

    Ok(ScheduleMessage {
        content: message_content_from_proto(content),
        headers: ScheduleHeaders::try_from(headers)?,
    })
}

fn message_content_from_proto(content: &content_v1alpha1::Content) -> MessageContent {
    if content.content_type == "application/json" {
        MessageContent::json(String::from_utf8_lossy(&content.data))
    } else {
        MessageContent::with_content_type(String::from_utf8_lossy(&content.data), content.content_type.clone())
    }
}

#[cfg(test)]
mod tests;
