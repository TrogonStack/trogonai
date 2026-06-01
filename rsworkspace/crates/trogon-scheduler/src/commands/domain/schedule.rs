use std::str::FromStr;

use chrono::{DateTime, Utc};
use trogon_nats::DottedNatsToken;
use trogonai_proto::convert::{PROTOBUF_DURATION_MAX_SECONDS, ProtobufDurationSeconds};

use crate::error::ScheduleSpecError;

use super::{
    MessageContent, MessageEnvelope, MessageHeader, MessageHeaders, MessageHeadersError, ScheduleEventDelivery,
    ScheduleEventSamplingSource, ScheduleEventSchedule, ScheduleEventStatus,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JobStatus {
    #[default]
    Enabled,
    Disabled,
}

impl JobStatus {
    pub fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Schedule {
    At {
        at: DateTime<Utc>,
    },
    Every {
        every_sec: EverySeconds,
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
    pub fn every(every_sec: u64) -> Result<Self, ScheduleSpecError> {
        Ok(Self::Every {
            every_sec: EverySeconds::new(every_sec)?,
        })
    }

    pub fn cron(expr: impl Into<String>, timezone: Option<String>) -> Result<Self, ScheduleSpecError> {
        Ok(Self::Cron {
            expr: CronExpression::new(expr)?,
            timezone: timezone.map(ScheduleTimezone::new).transpose()?,
        })
    }

    pub fn rrule(
        dtstart: impl Into<String>,
        rrule: impl Into<String>,
        timezone: Option<String>,
    ) -> Result<Self, ScheduleSpecError> {
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
pub struct EverySeconds(ProtobufDurationSeconds);

impl EverySeconds {
    pub fn new(every_sec: u64) -> Result<Self, ScheduleSpecError> {
        if every_sec == 0 {
            return Err(ScheduleSpecError::EverySecondsMustBePositive);
        }

        let Some(every_sec) = ProtobufDurationSeconds::new(every_sec) else {
            return Err(ScheduleSpecError::EverySecondsTooLarge {
                max: PROTOBUF_DURATION_MAX_SECONDS,
                actual: every_sec,
            });
        };

        Ok(Self(every_sec))
    }

    pub fn as_u64(self) -> u64 {
        self.0.as_u64()
    }

    pub fn as_protobuf_duration_seconds(self) -> ProtobufDurationSeconds {
        self.0
    }
}

impl TryFrom<u64> for EverySeconds {
    type Error = ScheduleSpecError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronExpression(String);

impl CronExpression {
    pub fn new(expr: impl Into<String>) -> Result<Self, ScheduleSpecError> {
        let expr = expr.into();
        cron::Schedule::from_str(&expr).map_err(|source| ScheduleSpecError::InvalidCronExpression {
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
    type Error = ScheduleSpecError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for CronExpression {
    type Error = ScheduleSpecError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RRuleExpression(String);

impl RRuleExpression {
    pub fn new(rrule: impl Into<String>) -> Result<Self, ScheduleSpecError> {
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
    type Error = ScheduleSpecError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for RRuleExpression {
    type Error = ScheduleSpecError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RRuleDateTime(String);

impl RRuleDateTime {
    pub fn new(field: &'static str, value: impl Into<String>) -> Result<Self, ScheduleSpecError> {
        let value = value.into();
        DateTime::parse_from_rfc3339(&value).map_err(|source| ScheduleSpecError::InvalidRRuleDateTime {
            field,
            value: value.clone(),
            source: Box::new(source),
        })?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }

    pub fn to_datetime(&self) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(&self.0)
            .expect("RRuleDateTime was validated as RFC3339 on construction")
            .with_timezone(&Utc)
    }
}

fn normalize_rrule_expression(raw: String) -> Result<String, ScheduleSpecError> {
    let trimmed = raw.trim();
    let without_prefix = strip_rrule_prefix(trimmed);

    if without_prefix.is_empty()
        || without_prefix.contains('\n')
        || without_prefix.contains('\r')
        || without_prefix.chars().any(char::is_control)
    {
        return Err(ScheduleSpecError::InvalidRRule {
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

fn validate_rrule_expression(rrule: &str) -> Result<(), ScheduleSpecError> {
    let mut has_count = false;
    let mut has_until = false;

    for part in rrule.split(';') {
        let Some((key, _)) = part.split_once('=') else {
            return Err(ScheduleSpecError::InvalidRRule {
                rrule: rrule.to_string(),
                source: Box::new(std::io::Error::other("RRULE parts must use KEY=VALUE")),
            });
        };
        match key {
            "COUNT" => has_count = true,
            "UNTIL" => has_until = true,
            "EXRULE" | "RSCALE" | "SKIP" => {
                return Err(ScheduleSpecError::InvalidRRule {
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
        return Err(ScheduleSpecError::InvalidRRule {
            rrule: rrule.to_string(),
            source: Box::new(std::io::Error::other("COUNT and UNTIL cannot be used together")),
        });
    }

    let set = format!("DTSTART:19700101T000000Z\nRRULE:{rrule}");
    rrule::RRuleSet::from_str(&set).map_err(|source| ScheduleSpecError::InvalidRRule {
        rrule: rrule.to_string(),
        source: Box::new(source),
    })?;

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeZone {
    id: String,
    tzdb_version: TzdbVersion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TzdbVersion(String);

pub type ScheduleTimezone = TimeZone;
pub type RRuleTimezone = TimeZone;

impl TimeZone {
    pub fn new(timezone: impl Into<String>) -> Result<Self, ScheduleSpecError> {
        Self::with_tzdb_version(timezone, TzdbVersion::current())
    }

    pub fn with_tzdb_version(
        timezone: impl Into<String>,
        tzdb_version: TzdbVersion,
    ) -> Result<Self, ScheduleSpecError> {
        let id = validate_timezone_token(timezone.into())?;
        chrono_tz::Tz::from_str(&id).map_err(|_| ScheduleSpecError::InvalidTimezone { timezone: id.clone() })?;

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
    type Error = ScheduleSpecError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for TimeZone {
    type Error = ScheduleSpecError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TzdbVersion {
    pub fn current() -> Self {
        Self(chrono_tz::IANA_TZDB_VERSION.to_string())
    }

    pub fn new(version: impl Into<String>) -> Result<Self, ScheduleSpecError> {
        let version = version.into();
        if is_valid_tzdb_version(&version) {
            Ok(Self(version))
        } else {
            Err(ScheduleSpecError::InvalidTimezoneDatabaseVersion { version })
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

fn validate_timezone_token(timezone: String) -> Result<String, ScheduleSpecError> {
    let trimmed = timezone.trim();
    if trimmed.is_empty() || trimmed != timezone || timezone.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
        return Err(ScheduleSpecError::InvalidTimezone { timezone });
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
pub struct JobHeaders(MessageHeaders);

impl JobHeaders {
    pub fn new<I, N, V>(headers: I) -> Result<Self, ScheduleSpecError>
    where
        I: IntoIterator<Item = (N, V)>,
        N: Into<String>,
        V: Into<String>,
    {
        let headers = MessageHeaders::new(headers).map_err(message_headers_error)?;
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

fn message_headers_error(source: MessageHeadersError) -> ScheduleSpecError {
    match source {
        MessageHeadersError::InvalidName { name } => ScheduleSpecError::InvalidHeaderName { name },
        MessageHeadersError::InvalidValue { name } => ScheduleSpecError::InvalidHeaderValue { name },
    }
}

impl TryFrom<MessageHeaders> for JobHeaders {
    type Error = ScheduleSpecError;

    fn try_from(value: MessageHeaders) -> Result<Self, Self::Error> {
        validate_reserved_scheduler_headers(value.as_slice())?;
        Ok(Self(value))
    }
}

impl From<JobHeaders> for MessageHeaders {
    fn from(value: JobHeaders) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobMessage {
    pub content: MessageContent,
    pub headers: JobHeaders,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryRoute(DottedNatsToken);

impl DeliveryRoute {
    pub fn new(route: impl AsRef<str>) -> Result<Self, ScheduleSpecError> {
        let route = route.as_ref();
        DottedNatsToken::new(route)
            .map(Self)
            .map_err(|source| ScheduleSpecError::InvalidRoute {
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
    type Error = ScheduleSpecError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for DeliveryRoute {
    type Error = ScheduleSpecError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SamplingSubject(DottedNatsToken);

impl SamplingSubject {
    pub fn new(subject: impl AsRef<str>) -> Result<Self, ScheduleSpecError> {
        let subject = subject.as_ref();
        DottedNatsToken::new(subject)
            .map(Self)
            .map_err(|source| ScheduleSpecError::InvalidSamplingSource {
                subject: subject.to_string(),
                source,
            })
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl TryFrom<String> for SamplingSubject {
    type Error = ScheduleSpecError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for SamplingSubject {
    type Error = ScheduleSpecError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TtlSeconds(ProtobufDurationSeconds);

impl TtlSeconds {
    pub fn new(ttl_sec: u64) -> Result<Self, ScheduleSpecError> {
        if ttl_sec == 0 {
            return Err(ScheduleSpecError::TtlMustBePositive);
        }

        let Some(ttl_sec) = ProtobufDurationSeconds::new(ttl_sec) else {
            return Err(ScheduleSpecError::TtlSecondsTooLarge {
                max: PROTOBUF_DURATION_MAX_SECONDS,
                actual: ttl_sec,
            });
        };

        Ok(Self(ttl_sec))
    }

    pub fn as_u64(self) -> u64 {
        self.0.as_u64()
    }

    pub fn as_protobuf_duration_seconds(self) -> ProtobufDurationSeconds {
        self.0
    }
}

impl TryFrom<u64> for TtlSeconds {
    type Error = ScheduleSpecError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SamplingSource {
    LatestFromSubject { subject: SamplingSubject },
}

impl SamplingSource {
    pub fn latest_from_subject(subject: impl AsRef<str>) -> Result<Self, ScheduleSpecError> {
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
        ttl_sec: Option<TtlSeconds>,
        source: Option<SamplingSource>,
    },
}

impl Delivery {
    pub fn nats_event(route: impl AsRef<str>) -> Result<Self, ScheduleSpecError> {
        Ok(Self::NatsEvent {
            route: DeliveryRoute::new(route)?,
            ttl_sec: None,
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

fn validate_reserved_scheduler_headers(headers: &[MessageHeader]) -> Result<(), ScheduleSpecError> {
    for header in headers {
        let name = header.name().as_str();
        if RESERVED_SCHEDULE_HEADERS
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(name))
        {
            return Err(ScheduleSpecError::ReservedHeaderName { name: name.to_string() });
        }
    }

    Ok(())
}

impl From<JobStatus> for ScheduleEventStatus {
    fn from(value: JobStatus) -> Self {
        match value {
            JobStatus::Enabled => Self::Scheduled,
            JobStatus::Disabled => Self::Paused,
        }
    }
}

impl From<ScheduleEventStatus> for JobStatus {
    fn from(value: ScheduleEventStatus) -> Self {
        match value {
            ScheduleEventStatus::Scheduled => Self::Enabled,
            ScheduleEventStatus::Paused => Self::Disabled,
        }
    }
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
            Schedule::Every { every_sec } => Self::Every { every_sec: *every_sec },
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
            Delivery::NatsEvent { route, ttl_sec, source } => Self::NatsMessage {
                subject: route.clone(),
                ttl_sec: *ttl_sec,
                source: source.as_ref().map(Into::into),
            },
        }
    }
}

impl From<JobMessage> for MessageEnvelope {
    fn from(value: JobMessage) -> Self {
        Self {
            content: value.content,
            headers: value.headers.into(),
        }
    }
}

impl From<&JobMessage> for MessageEnvelope {
    fn from(value: &JobMessage) -> Self {
        Self {
            content: value.content.clone(),
            headers: value.headers.clone().into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_status_helpers_work() {
        assert!(JobStatus::Enabled.is_enabled());
        assert_eq!(JobStatus::Enabled.as_str(), "enabled");
        assert!(!JobStatus::Disabled.is_enabled());
        assert_eq!(JobStatus::Disabled.as_str(), "disabled");
    }

    #[test]
    fn schedule_constructors_validate_and_preserve_values() {
        let cron = Schedule::cron("0 0 * * * *", Some("UTC".to_string())).unwrap();
        let rrule = Schedule::rrule(
            "2026-01-01T00:00:00Z",
            "RRULE:FREQ=DAILY;COUNT=2",
            Some("America/New_York".to_string()),
        )
        .unwrap();

        assert!(matches!(cron, Schedule::Cron { .. }));
        assert!(matches!(rrule, Schedule::RRule { .. }));
        assert!(Schedule::every(0).is_err());
        assert!(Schedule::cron("not a cron", None).is_err());
        assert!(Schedule::rrule("tomorrow", "FREQ=DAILY;COUNT=2", None).is_err());
        assert!(
            Schedule::rrule(
                "2026-01-01T00:00:00Z",
                "FREQ=DAILY;COUNT=2",
                Some("Nope/Zone".to_string())
            )
            .is_err()
        );
    }

    #[test]
    fn value_objects_cover_success_error_and_conversion_paths() {
        let every = EverySeconds::try_from(30).unwrap();
        let cron = CronExpression::try_from("0 0 * * * *".to_string()).unwrap();
        let rrule = RRuleExpression::try_from("freq=daily;count=2").unwrap();
        let dtstart = RRuleDateTime::new("dtstart", "2026-01-01T00:00:00Z").unwrap();
        let schedule_timezone = ScheduleTimezone::try_from("UTC").unwrap();
        let rrule_timezone = RRuleTimezone::try_from("UTC".to_string()).unwrap();

        assert_eq!(every.as_u64(), 30);
        assert!(EverySeconds::try_from(0).is_err());
        assert!(EverySeconds::try_from(PROTOBUF_DURATION_MAX_SECONDS + 1).is_err());
        assert_eq!(CronExpression::try_from("0 0 * * * *").unwrap().as_str(), "0 0 * * * *");
        assert_eq!(cron.as_str(), "0 0 * * * *");
        assert_eq!(cron.into_string(), "0 0 * * * *");
        assert_eq!(
            RRuleExpression::try_from("FREQ=DAILY;COUNT=2".to_string())
                .unwrap()
                .as_str(),
            "FREQ=DAILY;COUNT=2"
        );
        assert_eq!(rrule.as_str(), "FREQ=DAILY;COUNT=2");
        assert_eq!(rrule.into_string(), "FREQ=DAILY;COUNT=2");
        assert_eq!(dtstart.as_str(), "2026-01-01T00:00:00Z");
        assert_eq!(
            dtstart.to_datetime(),
            chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").unwrap()
        );
        assert_eq!(dtstart.into_string(), "2026-01-01T00:00:00Z");
        assert_eq!(schedule_timezone.as_str(), "UTC");
        assert_eq!(schedule_timezone.id(), "UTC");
        assert_eq!(schedule_timezone.tzdb_version().as_str(), chrono_tz::IANA_TZDB_VERSION);
        assert_eq!(schedule_timezone.into_string(), "UTC");
        assert_eq!(rrule_timezone.as_str(), "UTC");
        assert_eq!(rrule_timezone.tzdb_version().as_str(), chrono_tz::IANA_TZDB_VERSION);
        assert_eq!(rrule_timezone.into_string(), "UTC");
        let version = TzdbVersion::new("2025b").unwrap();
        assert_eq!(version.as_str(), "2025b");
        assert_eq!(version.clone().into_string(), "2025b");
        assert_eq!(
            TimeZone::with_tzdb_version("America/New_York", version)
                .unwrap()
                .tzdb_version()
                .as_str(),
            "2025b"
        );
        assert!(ScheduleTimezone::try_from("UTC\n").is_err());
        assert!(RRuleTimezone::try_from("Nope/Zone").is_err());
        assert!(TzdbVersion::new("").is_err());
        assert!(TzdbVersion::new("2025").is_err());
        assert!(TzdbVersion::new("25b").is_err());
        assert!(TzdbVersion::new("2025B").is_err());
    }

    #[test]
    fn rrule_validation_covers_invalid_shapes() {
        for raw in [
            "",
            "RRULE:",
            "FREQ=DAILY\nCOUNT=2",
            "FREQDAILY",
            "FREQ=DAILY;COUNT=2;UNTIL=20260101T000000Z",
            "FREQ=DAILY;EXRULE=FREQ=WEEKLY",
            "FREQ=DAILY;RSCALE=GREGORIAN",
            "FREQ=DAILY;SKIP=OMIT",
            "FREQ=NOPE;COUNT=2",
        ] {
            assert!(RRuleExpression::new(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn job_headers_cover_helpers_and_reserved_names() {
        let headers = JobHeaders::new([("x-kind", "heartbeat")]).unwrap();
        let message_headers = MessageHeaders::new([("x-kind", "heartbeat")]).unwrap();
        let from_message_headers = JobHeaders::try_from(message_headers).unwrap();

        assert!(!headers.is_empty());
        assert_eq!(headers.as_slice()[0].name().as_str(), "x-kind");
        assert_eq!(headers.clone().into_message_headers().as_slice().len(), 1);
        assert_eq!(MessageHeaders::from(from_message_headers).as_slice().len(), 1);
        assert!(JobHeaders::new([("Nats-Schedule", "value")]).is_err());
        assert!(JobHeaders::new([("bad name", "value")]).is_err());
        assert!(JobHeaders::new([("x-kind", "bad\nvalue")]).is_err());
    }

    #[test]
    fn route_sampling_and_delivery_cover_conversions() {
        let route = DeliveryRoute::try_from("agent.run".to_string()).unwrap();
        let route_ref = DeliveryRoute::try_from("agent.reply").unwrap();
        let subject = SamplingSubject::try_from("agent.events".to_string()).unwrap();
        let subject_ref = SamplingSubject::try_from("agent.replay").unwrap();
        let ttl = TtlSeconds::try_from(60).unwrap();
        let source = SamplingSource::latest_from_subject("agent.events").unwrap();
        let delivery = Delivery::NatsEvent {
            route: route.clone(),
            ttl_sec: Some(ttl),
            source: Some(source.clone()),
        };

        assert_eq!(route.as_str(), "agent.run");
        assert_eq!(route.as_token().as_str(), "agent.run");
        assert_eq!(route_ref.as_str(), "agent.reply");
        assert_eq!(subject.as_str(), "agent.events");
        assert_eq!(subject_ref.as_str(), "agent.replay");
        assert_eq!(ttl.as_u64(), 60);
        assert_eq!(ttl.as_protobuf_duration_seconds().as_u64(), 60);
        assert!(TtlSeconds::try_from(0).is_err());
        assert!(TtlSeconds::try_from(PROTOBUF_DURATION_MAX_SECONDS + 1).is_err());
        assert_eq!(source.subject().as_str(), "agent.events");
        assert!(DeliveryRoute::try_from("bad*route").is_err());
        assert!(SamplingSubject::try_from("bad>subject").is_err());
        assert!(matches!(
            Delivery::nats_event("agent.run").unwrap(),
            Delivery::NatsEvent { .. }
        ));
        assert!(matches!(
            ScheduleEventSamplingSource::from(source),
            ScheduleEventSamplingSource::LatestFromSubject { .. }
        ));
        assert!(matches!(
            ScheduleEventDelivery::from(delivery),
            ScheduleEventDelivery::NatsMessage {
                ttl_sec: Some(_),
                source: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn schedule_and_message_conversions_cover_all_variants() {
        let at = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let rrule_dt = RRuleDateTime::new("dtstart", "2026-01-01T00:00:00Z").unwrap();
        let schedules = [
            Schedule::At { at },
            Schedule::every(30).unwrap(),
            Schedule::cron("0 0 * * * *", Some("UTC".to_string())).unwrap(),
            Schedule::RRule {
                dtstart: rrule_dt.clone(),
                rrule: RRuleExpression::new("FREQ=DAILY;COUNT=2").unwrap(),
                timezone: Some(RRuleTimezone::new("UTC").unwrap()),
                rdate: vec![rrule_dt.clone()],
                exdate: vec![rrule_dt],
            },
        ];

        for schedule in schedules {
            let event_schedule = ScheduleEventSchedule::from(&schedule);
            let owned_event_schedule = ScheduleEventSchedule::from(schedule);
            assert_eq!(format!("{event_schedule:?}"), format!("{owned_event_schedule:?}"));
        }

        assert_eq!(JobStatus::from(ScheduleEventStatus::Scheduled), JobStatus::Enabled);
        assert_eq!(JobStatus::from(ScheduleEventStatus::Paused), JobStatus::Disabled);
        assert!(matches!(
            ScheduleEventStatus::from(JobStatus::Disabled),
            ScheduleEventStatus::Paused
        ));

        let message = JobMessage {
            content: MessageContent::json("{}"),
            headers: JobHeaders::new([("x-kind", "heartbeat")]).unwrap(),
        };
        assert_eq!(MessageEnvelope::from(&message).headers.as_slice().len(), 1);
        assert_eq!(
            MessageEnvelope::from(message).content.content_type().as_str(),
            "application/json"
        );
    }

    mod proptests {
        use super::super::RESERVED_SCHEDULE_HEADERS;
        use super::*;
        use proptest::prelude::*;

        const DOTTED_TOKEN_REGEX: &str = "[a-z][a-z0-9_-]{0,15}(\\.[a-z][a-z0-9_-]{0,15}){0,5}";

        #[derive(Debug, Clone)]
        enum DotShape {
            Leading,
            Trailing,
            Consecutive,
        }

        proptest! {
            #[test]
            fn every_seconds_accepts_any_positive_and_round_trips(n in 1u64..=PROTOBUF_DURATION_MAX_SECONDS) {
                let every = EverySeconds::new(n).unwrap();
                prop_assert_eq!(every.as_u64(), n);
            }

            #[test]
            fn ttl_seconds_accepts_any_positive_and_round_trips(n in 1u64..=PROTOBUF_DURATION_MAX_SECONDS) {
                let ttl = TtlSeconds::new(n).unwrap();
                prop_assert_eq!(ttl.as_u64(), n);
            }

            #[test]
            fn delivery_route_accepts_any_well_formed_dotted_token(s in DOTTED_TOKEN_REGEX) {
                let route = DeliveryRoute::new(&s).unwrap();
                prop_assert_eq!(route.as_str(), s.as_str());
            }

            #[test]
            fn delivery_route_rejects_any_string_with_wildcard_or_whitespace(
                prefix in "[a-z]{1,8}",
                bad in prop_oneof![Just('*'), Just('>'), Just(' '), Just('\t'), Just('\n')],
                suffix in "[a-z]{0,8}",
            ) {
                let s = format!("{prefix}{bad}{suffix}");
                prop_assert!(DeliveryRoute::new(&s).is_err());
            }

            #[test]
            fn delivery_route_rejects_dot_boundary_violations(
                core in "[a-z]+(\\.[a-z]+){0,3}",
                shape in prop_oneof![
                    Just(DotShape::Leading),
                    Just(DotShape::Trailing),
                    Just(DotShape::Consecutive),
                ],
            ) {
                let s = match shape {
                    DotShape::Leading => format!(".{core}"),
                    DotShape::Trailing => format!("{core}."),
                    DotShape::Consecutive => format!("{core}..tail"),
                };
                prop_assert!(DeliveryRoute::new(&s).is_err());
            }

            #[test]
            fn sampling_subject_accepts_any_well_formed_dotted_token(s in DOTTED_TOKEN_REGEX) {
                let subject = SamplingSubject::new(&s).unwrap();
                prop_assert_eq!(subject.as_str(), s.as_str());
            }

            #[test]
            fn sampling_subject_rejects_any_string_with_wildcard(
                prefix in "[a-z]{1,8}",
                wildcard in prop_oneof![Just('*'), Just('>')],
                suffix in "[a-z]{0,8}",
            ) {
                let s = format!("{prefix}{wildcard}{suffix}");
                prop_assert!(SamplingSubject::new(&s).is_err());
            }

            #[test]
            fn schedule_timezone_accepts_known_iana_zones(
                s in prop_oneof![Just("UTC".to_string()), Just("America/New_York".to_string()), Just("Europe/London".to_string())],
            ) {
                let tz = ScheduleTimezone::new(&s).unwrap();
                prop_assert_eq!(tz.as_str(), s.as_str());
            }

            #[test]
            fn schedule_timezone_rejects_any_string_containing_whitespace(
                prefix in "[A-Za-z]{1,8}",
                ws in prop_oneof![Just(' '), Just('\t'), Just('\n')],
                suffix in "[A-Za-z]{0,8}",
            ) {
                let s = format!("{prefix}{ws}{suffix}");
                prop_assert!(ScheduleTimezone::new(&s).is_err());
            }

            #[test]
            fn reserved_scheduler_headers_are_rejected_in_any_case(
                name_template in proptest::sample::select(&RESERVED_SCHEDULE_HEADERS[..]),
                upper_flags in proptest::collection::vec(any::<bool>(), 32),
                value in "[ -~]{0,16}",
            ) {
                let name: String = name_template
                    .chars()
                    .zip(upper_flags.iter())
                    .map(|(ch, &upper)| if upper { ch.to_ascii_uppercase() } else { ch.to_ascii_lowercase() })
                    .collect();

                let result = JobHeaders::new([(name, value)]);
                let is_reserved_error = matches!(result, Err(ScheduleSpecError::ReservedHeaderName { .. }));
                prop_assert!(is_reserved_error);
            }

            #[test]
            fn non_reserved_headers_construct_and_preserve_input(
                name in "x-[a-z]{1,12}",
                value in "[ -~]{0,16}",
            ) {
                let headers = JobHeaders::new([(name.clone(), value.clone())]).unwrap();
                let slice = headers.as_slice();
                prop_assert_eq!(slice.len(), 1);
                prop_assert_eq!(slice[0].name().as_str(), name.as_str());
                prop_assert_eq!(slice[0].value().as_str(), value.as_str());
            }

            #[test]
            fn schedule_every_constructor_matches_value_object(n in 1u64..=PROTOBUF_DURATION_MAX_SECONDS) {
                let schedule = Schedule::every(n).unwrap();
                match schedule {
                    Schedule::Every { every_sec } => prop_assert_eq!(every_sec.as_u64(), n),
                    other => prop_assert!(false, "expected Every, got {other:?}"),
                }
            }
        }
    }
}
