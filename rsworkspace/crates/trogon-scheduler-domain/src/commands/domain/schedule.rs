use std::{str::FromStr, time::Duration};

use crate::subject::DottedNatsToken;
use chrono::{DateTime, Utc};
use trogonai_proto::convert::PROTOBUF_DURATION_MAX;

use super::{
    MessageContent, MessageEnvelope, MessageHeader, MessageHeaders, MessageHeadersError, ScheduleEventDelivery,
    ScheduleEventSamplingSource, ScheduleEventSchedule,
};

#[derive(Debug, thiserror::Error)]
pub enum ScheduleError {
    #[error("{0}")]
    CronExpression(
        #[from]
        #[source]
        CronExpressionError,
    ),
    #[error("{0}")]
    RRuleDateTime(
        #[from]
        #[source]
        RRuleDateTimeError,
    ),
    #[error("{0}")]
    RRuleExpression(
        #[from]
        #[source]
        RRuleExpressionError,
    ),
    #[error("{0}")]
    TimeZone(
        #[from]
        #[source]
        TimeZoneError,
    ),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Schedule {
    At {
        at: DateTime<Utc>,
    },
    Every {
        every: EveryDuration,
    },
    Cron {
        expr: CronExpression,
        timezone: Option<ScheduleTimezone>,
    },
    RRule {
        dtstart: RRuleDateTime,
        rrule: RRuleExpression,
        timezone: Option<RRuleTimezone>,
        rdate: Vec<RRuleDateTime>,
        exdate: Vec<RRuleDateTime>,
    },
}

impl Schedule {
    pub fn every(every: Duration) -> Result<Self, EveryDurationError> {
        Ok(Self::Every {
            every: EveryDuration::new(every)?,
        })
    }

    pub fn cron(expr: impl Into<String>, timezone: Option<String>) -> Result<Self, ScheduleError> {
        Ok(Self::Cron {
            expr: CronExpression::new(expr)?,
            timezone: timezone.map(ScheduleTimezone::new).transpose()?,
        })
    }

    pub fn rrule(
        dtstart: impl Into<String>,
        rrule: impl Into<String>,
        timezone: Option<String>,
    ) -> Result<Self, ScheduleError> {
        Ok(Self::RRule {
            dtstart: RRuleDateTime::new("dtstart", dtstart)?,
            rrule: RRuleExpression::new(rrule)?,
            timezone: timezone.map(RRuleTimezone::new).transpose()?,
            rdate: Vec::new(),
            exdate: Vec::new(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EveryDuration(Duration);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EveryDurationError {
    #[error("every duration must be positive")]
    MustBePositive,
    #[error("every duration must be at most {max:?}, got {actual:?}")]
    TooLarge { max: Duration, actual: Duration },
}

impl EveryDuration {
    pub fn new(every: Duration) -> Result<Self, EveryDurationError> {
        if every.is_zero() {
            return Err(EveryDurationError::MustBePositive);
        }
        if every > PROTOBUF_DURATION_MAX {
            return Err(EveryDurationError::TooLarge {
                max: PROTOBUF_DURATION_MAX,
                actual: every,
            });
        }

        Ok(Self(every))
    }

    pub fn from_secs(seconds: u64) -> Result<Self, EveryDurationError> {
        Self::new(Duration::from_secs(seconds))
    }

    pub fn as_duration(self) -> Duration {
        self.0
    }
}

impl TryFrom<Duration> for EveryDuration {
    type Error = EveryDurationError;

    fn try_from(value: Duration) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<u64> for EveryDuration {
    type Error = EveryDurationError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::from_secs(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronExpression(String);

#[derive(Debug, thiserror::Error)]
pub enum CronExpressionError {
    #[error("cron expression '{expr}' is invalid: {source}")]
    Invalid {
        expr: String,
        #[source]
        source: Box<dyn std::error::Error>,
    },
}

impl CronExpression {
    pub fn new(expr: impl Into<String>) -> Result<Self, CronExpressionError> {
        let expr = expr.into();
        cron::Schedule::from_str(&expr).map_err(|source| CronExpressionError::Invalid {
            expr: expr.clone(),
            source: Box::new(source),
        })?;
        Ok(Self(expr))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl TryFrom<String> for CronExpression {
    type Error = CronExpressionError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for CronExpression {
    type Error = CronExpressionError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RRuleExpression(String);

#[derive(Debug, thiserror::Error)]
pub enum RRuleExpressionError {
    #[error("rrule '{rrule}' is invalid: {source}")]
    Invalid {
        rrule: String,
        #[source]
        source: Box<dyn std::error::Error>,
    },
}

impl RRuleExpression {
    pub fn new(rrule: impl Into<String>) -> Result<Self, RRuleExpressionError> {
        let rrule = normalize_rrule_expression(rrule.into())?;
        validate_rrule_expression(&rrule)?;
        Ok(Self(rrule))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl TryFrom<String> for RRuleExpression {
    type Error = RRuleExpressionError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for RRuleExpression {
    type Error = RRuleExpressionError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RRuleDateTime {
    raw: String,
    datetime: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum RRuleDateTimeError {
    #[error("{field} datetime '{value}' is invalid: {source}")]
    Invalid {
        field: &'static str,
        value: String,
        #[source]
        source: Box<dyn std::error::Error>,
    },
}

impl RRuleDateTime {
    pub fn new(field: &'static str, value: impl Into<String>) -> Result<Self, RRuleDateTimeError> {
        let value = value.into();
        let datetime = DateTime::parse_from_rfc3339(&value)
            .map(|datetime| datetime.with_timezone(&Utc))
            .map_err(|source| RRuleDateTimeError::Invalid {
                field,
                value: value.clone(),
                source: Box::new(source),
            })?;
        Ok(Self { raw: value, datetime })
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    pub fn into_string(self) -> String {
        self.raw
    }

    pub fn to_datetime(&self) -> DateTime<Utc> {
        self.datetime
    }
}

fn normalize_rrule_expression(raw: String) -> Result<String, RRuleExpressionError> {
    let trimmed = raw.trim();
    let without_prefix = strip_rrule_prefix(trimmed);

    if without_prefix.is_empty()
        || without_prefix.contains('\n')
        || without_prefix.contains('\r')
        || without_prefix.chars().any(char::is_control)
    {
        return Err(RRuleExpressionError::Invalid {
            rrule: raw,
            source: Box::new(std::io::Error::other("empty or multi-line RRULE")),
        });
    }

    Ok(without_prefix.to_ascii_uppercase())
}

fn strip_rrule_prefix(trimmed: &str) -> &str {
    let prefix_len = "RRULE:".len();
    if trimmed
        .get(..prefix_len)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("RRULE:"))
    {
        trimmed.get(prefix_len..).unwrap_or_default().trim()
    } else {
        trimmed
    }
}

fn validate_rrule_expression(rrule: &str) -> Result<(), RRuleExpressionError> {
    let mut has_count = false;
    let mut has_until = false;

    for part in rrule.split(';') {
        let Some((key, _)) = part.split_once('=') else {
            return Err(RRuleExpressionError::Invalid {
                rrule: rrule.to_string(),
                source: Box::new(std::io::Error::other("RRULE parts must use KEY=VALUE")),
            });
        };
        match key {
            "COUNT" => has_count = true,
            "UNTIL" => has_until = true,
            "EXRULE" | "RSCALE" | "SKIP" => {
                return Err(RRuleExpressionError::Invalid {
                    rrule: rrule.to_string(),
                    source: Box::new(std::io::Error::other(format!(
                        "{key} is not supported by cron RRULE schedules"
                    ))),
                });
            }
            _ => {}
        }
    }

    if has_count && has_until {
        return Err(RRuleExpressionError::Invalid {
            rrule: rrule.to_string(),
            source: Box::new(std::io::Error::other("COUNT and UNTIL cannot be used together")),
        });
    }

    #[cfg(feature = "schedule-validation")]
    {
        let set = format!("DTSTART:19700101T000000Z\nRRULE:{rrule}");
        rrule::RRuleSet::from_str(&set).map_err(|source| RRuleExpressionError::Invalid {
            rrule: rrule.to_string(),
            source: Box::new(source),
        })?;
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeZone {
    id: String,
    tzdb_version: TzdbVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TimeZoneError {
    #[error("timezone '{timezone}' is invalid")]
    Invalid { timezone: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TzdbVersion(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TzdbVersionError {
    #[error("timezone database version '{version}' is invalid")]
    Invalid { version: String },
}

pub type ScheduleTimezone = TimeZone;
pub type RRuleTimezone = TimeZone;

impl TimeZone {
    pub fn new(timezone: impl Into<String>) -> Result<Self, TimeZoneError> {
        Self::with_tzdb_version(timezone, TzdbVersion::current())
    }

    pub fn with_tzdb_version(timezone: impl Into<String>, tzdb_version: TzdbVersion) -> Result<Self, TimeZoneError> {
        let id = validate_timezone_token(timezone.into())?;
        #[cfg(feature = "schedule-validation")]
        chrono_tz::Tz::from_str(&id).map_err(|_| TimeZoneError::Invalid { timezone: id.clone() })?;

        Ok(Self { id, tzdb_version })
    }

    pub fn as_str(&self) -> &str {
        self.id()
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn tzdb_version(&self) -> &TzdbVersion {
        &self.tzdb_version
    }

    pub fn into_string(self) -> String {
        self.id
    }
}

impl TryFrom<String> for TimeZone {
    type Error = TimeZoneError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for TimeZone {
    type Error = TimeZoneError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TzdbVersion {
    pub fn current() -> Self {
        #[cfg(feature = "schedule-validation")]
        {
            Self(chrono_tz::IANA_TZDB_VERSION.to_string())
        }
        #[cfg(not(feature = "schedule-validation"))]
        {
            Self("wasm-guest".to_string())
        }
    }

    pub fn new(version: impl Into<String>) -> Result<Self, TzdbVersionError> {
        let version = version.into();
        if is_valid_tzdb_version(&version) {
            Ok(Self(version))
        } else {
            Err(TzdbVersionError::Invalid { version })
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

fn validate_timezone_token(timezone: String) -> Result<String, TimeZoneError> {
    let trimmed = timezone.trim();
    if trimmed.is_empty() || trimmed != timezone || timezone.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
        return Err(TimeZoneError::Invalid { timezone });
    }

    Ok(timezone)
}

fn is_valid_tzdb_version(version: &str) -> bool {
    let Some(year) = version.get(..4) else {
        return false;
    };
    let suffix = &version[4..];

    year.chars().all(|ch| ch.is_ascii_digit()) && !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_lowercase())
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScheduleHeaders(MessageHeaders);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleHeadersError {
    #[error("{source}")]
    MessageHeaders {
        #[from]
        #[source]
        source: MessageHeadersError,
    },
    #[error("header name '{name}' is reserved")]
    ReservedName { name: String },
}

impl ScheduleHeaders {
    pub fn new<I, N, V>(headers: I) -> Result<Self, ScheduleHeadersError>
    where
        I: IntoIterator<Item = (N, V)>,
        N: Into<String>,
        V: Into<String>,
    {
        let headers = MessageHeaders::new(headers).map_err(|source| ScheduleHeadersError::MessageHeaders { source })?;
        Self::try_from(headers)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_slice(&self) -> &[MessageHeader] {
        self.0.as_slice()
    }

    pub fn into_message_headers(self) -> MessageHeaders {
        self.0
    }
}

impl TryFrom<MessageHeaders> for ScheduleHeaders {
    type Error = ScheduleHeadersError;

    fn try_from(value: MessageHeaders) -> Result<Self, Self::Error> {
        validate_reserved_scheduler_headers(value.as_slice())?;
        Ok(Self(value))
    }
}

impl From<ScheduleHeaders> for MessageHeaders {
    fn from(value: ScheduleHeaders) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScheduleMessage {
    pub content: MessageContent,
    pub headers: ScheduleHeaders,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryRoute(DottedNatsToken);

#[derive(Debug, thiserror::Error)]
pub enum DeliveryRouteError {
    #[error("delivery route '{route}' is invalid: {source}")]
    Invalid {
        route: String,
        #[source]
        source: crate::subject::SubjectTokenViolation,
    },
}

impl DeliveryRoute {
    pub fn new(route: impl AsRef<str>) -> Result<Self, DeliveryRouteError> {
        let route = route.as_ref();
        DottedNatsToken::new(route)
            .map(Self)
            .map_err(|source| DeliveryRouteError::Invalid {
                route: route.to_string(),
                source,
            })
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn as_token(&self) -> &DottedNatsToken {
        &self.0
    }
}

impl TryFrom<String> for DeliveryRoute {
    type Error = DeliveryRouteError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for DeliveryRoute {
    type Error = DeliveryRouteError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SamplingSubject(DottedNatsToken);

#[derive(Debug, thiserror::Error)]
pub enum SamplingSubjectError {
    #[error("sampling subject '{subject}' is invalid: {source}")]
    Invalid {
        subject: String,
        #[source]
        source: crate::subject::SubjectTokenViolation,
    },
}

impl SamplingSubject {
    pub fn new(subject: impl AsRef<str>) -> Result<Self, SamplingSubjectError> {
        let subject = subject.as_ref();
        DottedNatsToken::new(subject)
            .map(Self)
            .map_err(|source| SamplingSubjectError::Invalid {
                subject: subject.to_string(),
                source,
            })
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl TryFrom<String> for SamplingSubject {
    type Error = SamplingSubjectError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for SamplingSubject {
    type Error = SamplingSubjectError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TtlDuration(Duration);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TtlDurationError {
    #[error("ttl duration must be positive")]
    MustBePositive,
    #[error("ttl duration must be at most {max:?}, got {actual:?}")]
    TooLarge { max: Duration, actual: Duration },
}

impl TtlDuration {
    pub fn new(ttl: Duration) -> Result<Self, TtlDurationError> {
        if ttl.is_zero() {
            return Err(TtlDurationError::MustBePositive);
        }
        if ttl > PROTOBUF_DURATION_MAX {
            return Err(TtlDurationError::TooLarge {
                max: PROTOBUF_DURATION_MAX,
                actual: ttl,
            });
        }

        Ok(Self(ttl))
    }

    pub fn from_secs(seconds: u64) -> Result<Self, TtlDurationError> {
        Self::new(Duration::from_secs(seconds))
    }

    pub fn as_duration(self) -> Duration {
        self.0
    }
}

impl TryFrom<Duration> for TtlDuration {
    type Error = TtlDurationError;

    fn try_from(value: Duration) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<u64> for TtlDuration {
    type Error = TtlDurationError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::from_secs(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SamplingSource {
    LatestFromSubject { subject: SamplingSubject },
}

impl SamplingSource {
    pub fn latest_from_subject(subject: impl AsRef<str>) -> Result<Self, SamplingSubjectError> {
        Ok(Self::LatestFromSubject {
            subject: SamplingSubject::new(subject)?,
        })
    }

    pub fn subject(&self) -> &SamplingSubject {
        match self {
            Self::LatestFromSubject { subject } => subject,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    NatsEvent {
        route: DeliveryRoute,
        ttl: Option<TtlDuration>,
        source: Option<SamplingSource>,
    },
}

impl Delivery {
    pub fn nats_event(route: impl AsRef<str>) -> Result<Self, DeliveryRouteError> {
        Ok(Self::NatsEvent {
            route: DeliveryRoute::new(route)?,
            ttl: None,
            source: None,
        })
    }
}

const RESERVED_SCHEDULE_HEADERS: [&str; 5] = [
    "Nats-Schedule",
    "Nats-Schedule-Source",
    "Nats-Schedule-Target",
    "Nats-Schedule-Time-Zone",
    "Nats-Schedule-TTL",
];

fn validate_reserved_scheduler_headers(headers: &[MessageHeader]) -> Result<(), ScheduleHeadersError> {
    for header in headers {
        let name = header.name().as_str();
        if RESERVED_SCHEDULE_HEADERS
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(name))
        {
            return Err(ScheduleHeadersError::ReservedName { name: name.to_string() });
        }
    }

    Ok(())
}

impl From<Schedule> for ScheduleEventSchedule {
    fn from(value: Schedule) -> Self {
        ScheduleEventSchedule::from(&value)
    }
}

impl From<&Schedule> for ScheduleEventSchedule {
    fn from(value: &Schedule) -> Self {
        match value {
            Schedule::At { at } => Self::At { at: *at },
            Schedule::Every { every } => Self::Every { every: *every },
            Schedule::Cron { expr, timezone } => Self::Cron {
                expr: expr.clone(),
                timezone: timezone.clone(),
            },
            Schedule::RRule {
                dtstart,
                rrule,
                timezone,
                rdate,
                exdate,
            } => Self::RRule {
                dtstart: dtstart.to_datetime(),
                rrule: rrule.clone(),
                timezone: timezone.clone(),
                rdate: rdate.iter().map(RRuleDateTime::to_datetime).collect(),
                exdate: exdate.iter().map(RRuleDateTime::to_datetime).collect(),
            },
        }
    }
}

impl From<SamplingSource> for ScheduleEventSamplingSource {
    fn from(value: SamplingSource) -> Self {
        ScheduleEventSamplingSource::from(&value)
    }
}

impl From<&SamplingSource> for ScheduleEventSamplingSource {
    fn from(value: &SamplingSource) -> Self {
        match value {
            SamplingSource::LatestFromSubject { subject } => Self::LatestFromSubject {
                subject: subject.clone(),
            },
        }
    }
}

impl From<Delivery> for ScheduleEventDelivery {
    fn from(value: Delivery) -> Self {
        ScheduleEventDelivery::from(&value)
    }
}

impl From<&Delivery> for ScheduleEventDelivery {
    fn from(value: &Delivery) -> Self {
        match value {
            Delivery::NatsEvent { route, ttl, source } => Self::NatsMessage {
                subject: route.clone(),
                ttl: *ttl,
                source: source.as_ref().map(Into::into),
            },
        }
    }
}

impl From<ScheduleMessage> for MessageEnvelope {
    fn from(value: ScheduleMessage) -> Self {
        Self {
            content: value.content,
            headers: value.headers.into(),
        }
    }
}

impl From<&ScheduleMessage> for MessageEnvelope {
    fn from(value: &ScheduleMessage) -> Self {
        Self {
            content: value.content.clone(),
            headers: value.headers.clone().into(),
        }
    }
}

#[cfg(test)]
mod tests;
