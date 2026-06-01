use std::{str::FromStr, time::Duration};

use chrono::{DateTime, Utc};
use trogon_nats::DottedNatsToken;
use trogonai_proto::convert::PROTOBUF_DURATION_MAX;

use super::{
    MessageContent, MessageEnvelope, MessageHeader, MessageHeaders, MessageHeadersError, ScheduleEventDelivery,
    ScheduleEventSamplingSource, ScheduleEventSchedule,
};

#[derive(Debug)]
pub enum ScheduleError {
    CronExpression(CronExpressionError),
    RRuleDateTime(RRuleDateTimeError),
    RRuleExpression(RRuleExpressionError),
    TimeZone(TimeZoneError),
}

impl std::fmt::Display for ScheduleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CronExpression(source) => source.fmt(formatter),
            Self::RRuleDateTime(source) => source.fmt(formatter),
            Self::RRuleExpression(source) => source.fmt(formatter),
            Self::TimeZone(source) => source.fmt(formatter),
        }
    }
}

impl std::error::Error for ScheduleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CronExpression(source) => Some(source),
            Self::RRuleDateTime(source) => Some(source),
            Self::RRuleExpression(source) => Some(source),
            Self::TimeZone(source) => Some(source),
        }
    }
}

impl From<CronExpressionError> for ScheduleError {
    fn from(source: CronExpressionError) -> Self {
        Self::CronExpression(source)
    }
}

impl From<RRuleDateTimeError> for ScheduleError {
    fn from(source: RRuleDateTimeError) -> Self {
        Self::RRuleDateTime(source)
    }
}

impl From<RRuleExpressionError> for ScheduleError {
    fn from(source: RRuleExpressionError) -> Self {
        Self::RRuleExpression(source)
    }
}

impl From<TimeZoneError> for ScheduleError {
    fn from(source: TimeZoneError) -> Self {
        Self::TimeZone(source)
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EveryDurationError {
    MustBePositive,
    TooLarge { max: Duration, actual: Duration },
}

impl std::fmt::Display for EveryDurationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MustBePositive => formatter.write_str("every duration must be positive"),
            Self::TooLarge { max, actual } => {
                write!(formatter, "every duration must be at most {max:?}, got {actual:?}")
            }
        }
    }
}

impl std::error::Error for EveryDurationError {}

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

#[derive(Debug)]
pub enum CronExpressionError {
    Invalid {
        expr: String,
        source: Box<dyn std::error::Error>,
    },
}

impl std::fmt::Display for CronExpressionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid { expr, source } => write!(formatter, "cron expression '{expr}' is invalid: {source}"),
        }
    }
}

impl std::error::Error for CronExpressionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Invalid { source, .. } => Some(source.as_ref()),
        }
    }
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

#[derive(Debug)]
pub enum RRuleExpressionError {
    Invalid {
        rrule: String,
        source: Box<dyn std::error::Error>,
    },
}

impl std::fmt::Display for RRuleExpressionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid { rrule, source } => write!(formatter, "rrule '{rrule}' is invalid: {source}"),
        }
    }
}

impl std::error::Error for RRuleExpressionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Invalid { source, .. } => Some(source.as_ref()),
        }
    }
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
pub struct RRuleDateTime(String);

#[derive(Debug)]
pub enum RRuleDateTimeError {
    Invalid {
        field: &'static str,
        value: String,
        source: Box<dyn std::error::Error>,
    },
}

impl std::fmt::Display for RRuleDateTimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid { field, value, source } => {
                write!(formatter, "{field} datetime '{value}' is invalid: {source}")
            }
        }
    }
}

impl std::error::Error for RRuleDateTimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Invalid { source, .. } => Some(source.as_ref()),
        }
    }
}

impl RRuleDateTime {
    pub fn new(field: &'static str, value: impl Into<String>) -> Result<Self, RRuleDateTimeError> {
        let value = value.into();
        DateTime::parse_from_rfc3339(&value).map_err(|source| RRuleDateTimeError::Invalid {
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

    let set = format!("DTSTART:19700101T000000Z\nRRULE:{rrule}");
    rrule::RRuleSet::from_str(&set).map_err(|source| RRuleExpressionError::Invalid {
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
pub enum TimeZoneError {
    Invalid { timezone: String },
}

impl std::fmt::Display for TimeZoneError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid { timezone } => write!(formatter, "timezone '{timezone}' is invalid"),
        }
    }
}

impl std::error::Error for TimeZoneError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TzdbVersion(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TzdbVersionError {
    Invalid { version: String },
}

impl std::fmt::Display for TzdbVersionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid { version } => write!(formatter, "timezone database version '{version}' is invalid"),
        }
    }
}

impl std::error::Error for TzdbVersionError {}

pub type ScheduleTimezone = TimeZone;
pub type RRuleTimezone = TimeZone;

impl TimeZone {
    pub fn new(timezone: impl Into<String>) -> Result<Self, TimeZoneError> {
        Self::with_tzdb_version(timezone, TzdbVersion::current())
    }

    pub fn with_tzdb_version(timezone: impl Into<String>, tzdb_version: TzdbVersion) -> Result<Self, TimeZoneError> {
        let id = validate_timezone_token(timezone.into())?;
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
        Self(chrono_tz::IANA_TZDB_VERSION.to_string())
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleHeadersError {
    MessageHeaders { source: MessageHeadersError },
    ReservedName { name: String },
}

impl std::fmt::Display for ScheduleHeadersError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MessageHeaders { source } => source.fmt(formatter),
            Self::ReservedName { name } => write!(formatter, "header name '{name}' is reserved"),
        }
    }
}

impl std::error::Error for ScheduleHeadersError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MessageHeaders { source } => Some(source),
            Self::ReservedName { .. } => None,
        }
    }
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

#[derive(Debug)]
pub enum DeliveryRouteError {
    Invalid {
        route: String,
        source: trogon_nats::SubjectTokenViolation,
    },
}

impl std::fmt::Display for DeliveryRouteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid { route, source } => write!(formatter, "delivery route '{route}' is invalid: {source}"),
        }
    }
}

impl std::error::Error for DeliveryRouteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Invalid { source, .. } => Some(source),
        }
    }
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

#[derive(Debug)]
pub enum SamplingSubjectError {
    Invalid {
        subject: String,
        source: trogon_nats::SubjectTokenViolation,
    },
}

impl std::fmt::Display for SamplingSubjectError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid { subject, source } => {
                write!(formatter, "sampling subject '{subject}' is invalid: {source}")
            }
        }
    }
}

impl std::error::Error for SamplingSubjectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Invalid { source, .. } => Some(source),
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtlDurationError {
    MustBePositive,
    TooLarge { max: Duration, actual: Duration },
}

impl std::fmt::Display for TtlDurationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MustBePositive => formatter.write_str("ttl duration must be positive"),
            Self::TooLarge { max, actual } => {
                write!(formatter, "ttl duration must be at most {max:?}, got {actual:?}")
            }
        }
    }
}

impl std::error::Error for TtlDurationError {}

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
mod tests {
    use std::error::Error;

    use trogonai_proto::convert::PROTOBUF_DURATION_MAX_SECONDS;

    use super::*;

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
        assert!(Schedule::every(Duration::ZERO).is_err());
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
    fn errors_are_scoped_to_the_value_that_failed_to_construct() {
        let every_zero = EveryDuration::new(Duration::ZERO).unwrap_err();
        assert_eq!(every_zero, EveryDurationError::MustBePositive);
        assert_eq!(every_zero.to_string(), "every duration must be positive");
        assert!(every_zero.source().is_none());

        let every_too_large = EveryDuration::new(PROTOBUF_DURATION_MAX + Duration::from_nanos(1)).unwrap_err();
        assert_eq!(
            every_too_large,
            EveryDurationError::TooLarge {
                max: PROTOBUF_DURATION_MAX,
                actual: PROTOBUF_DURATION_MAX + Duration::from_nanos(1),
            }
        );
        assert!(
            every_too_large
                .to_string()
                .starts_with("every duration must be at most")
        );
        assert!(every_too_large.source().is_none());

        let cron = CronExpression::new("not a cron").unwrap_err();
        assert!(cron.to_string().starts_with("cron expression 'not a cron' is invalid:"));
        assert!(cron.source().is_some());

        let rrule_date = RRuleDateTime::new("dtstart", "tomorrow").unwrap_err();
        assert!(
            rrule_date
                .to_string()
                .starts_with("dtstart datetime 'tomorrow' is invalid:")
        );
        assert!(rrule_date.source().is_some());

        let rrule = RRuleExpression::new("FREQDAILY").unwrap_err();
        assert_eq!(
            rrule.to_string(),
            "rrule 'FREQDAILY' is invalid: RRULE parts must use KEY=VALUE"
        );
        assert!(rrule.source().is_some());

        let timezone = TimeZone::new("Nope/Zone").unwrap_err();
        assert_eq!(
            timezone,
            TimeZoneError::Invalid {
                timezone: "Nope/Zone".to_string()
            }
        );
        assert_eq!(timezone.to_string(), "timezone 'Nope/Zone' is invalid");
        assert!(timezone.source().is_none());

        let tzdb_version = TzdbVersion::new("2025").unwrap_err();
        assert_eq!(
            tzdb_version,
            TzdbVersionError::Invalid {
                version: "2025".to_string()
            }
        );
        assert_eq!(tzdb_version.to_string(), "timezone database version '2025' is invalid");
        assert!(tzdb_version.source().is_none());

        let header = ScheduleHeaders::new([("bad name", "value")]).unwrap_err();
        assert_eq!(header.to_string(), "header name 'bad name' is invalid");
        assert!(header.source().is_some());

        let reserved_header = ScheduleHeaders::new([("Nats-Schedule", "value")]).unwrap_err();
        assert_eq!(
            reserved_header,
            ScheduleHeadersError::ReservedName {
                name: "Nats-Schedule".to_string()
            }
        );
        assert_eq!(reserved_header.to_string(), "header name 'Nats-Schedule' is reserved");
        assert!(reserved_header.source().is_none());

        let route = DeliveryRoute::new("bad*route").unwrap_err();
        assert!(route.to_string().starts_with("delivery route 'bad*route' is invalid:"));
        assert!(route.source().is_some());

        let subject = SamplingSubject::new("bad>subject").unwrap_err();
        assert!(
            subject
                .to_string()
                .starts_with("sampling subject 'bad>subject' is invalid:")
        );
        assert!(subject.source().is_some());

        let ttl_zero = TtlDuration::new(Duration::ZERO).unwrap_err();
        assert_eq!(ttl_zero, TtlDurationError::MustBePositive);
        assert_eq!(ttl_zero.to_string(), "ttl duration must be positive");
        assert!(ttl_zero.source().is_none());

        let ttl_too_large = TtlDuration::new(PROTOBUF_DURATION_MAX + Duration::from_nanos(1)).unwrap_err();
        assert_eq!(
            ttl_too_large,
            TtlDurationError::TooLarge {
                max: PROTOBUF_DURATION_MAX,
                actual: PROTOBUF_DURATION_MAX + Duration::from_nanos(1),
            }
        );
        assert!(ttl_too_large.to_string().starts_with("ttl duration must be at most"));
        assert!(ttl_too_large.source().is_none());
    }

    #[test]
    fn schedule_convenience_errors_only_wrap_schedule_value_failures() {
        let cron = Schedule::cron("not a cron", None).unwrap_err();
        assert!(matches!(cron, ScheduleError::CronExpression(_)));
        assert!(cron.to_string().starts_with("cron expression 'not a cron' is invalid:"));
        assert!(cron.source().is_some());

        let cron_timezone = Schedule::cron("0 0 * * * *", Some("Nope/Zone".to_string())).unwrap_err();
        assert!(matches!(cron_timezone, ScheduleError::TimeZone(_)));
        assert_eq!(cron_timezone.to_string(), "timezone 'Nope/Zone' is invalid");
        assert!(cron_timezone.source().is_some());

        let rrule_dtstart = Schedule::rrule("tomorrow", "FREQ=DAILY;COUNT=2", None).unwrap_err();
        assert!(matches!(rrule_dtstart, ScheduleError::RRuleDateTime(_)));
        assert!(
            rrule_dtstart
                .to_string()
                .starts_with("dtstart datetime 'tomorrow' is invalid:")
        );
        assert!(rrule_dtstart.source().is_some());

        let rrule = Schedule::rrule("2026-01-01T00:00:00Z", "FREQDAILY", None).unwrap_err();
        assert!(matches!(rrule, ScheduleError::RRuleExpression(_)));
        assert_eq!(
            rrule.to_string(),
            "rrule 'FREQDAILY' is invalid: RRULE parts must use KEY=VALUE"
        );
        assert!(rrule.source().is_some());
    }

    #[test]
    fn value_objects_cover_success_error_and_conversion_paths() {
        let every = EveryDuration::new(Duration::from_secs(30)).unwrap();
        let cron = CronExpression::try_from("0 0 * * * *".to_string()).unwrap();
        let rrule = RRuleExpression::try_from("freq=daily;count=2").unwrap();
        let dtstart = RRuleDateTime::new("dtstart", "2026-01-01T00:00:00Z").unwrap();
        let schedule_timezone = ScheduleTimezone::try_from("UTC").unwrap();
        let rrule_timezone = RRuleTimezone::try_from("UTC".to_string()).unwrap();

        assert_eq!(every.as_duration(), Duration::from_secs(30));
        assert_eq!(
            EveryDuration::from_secs(31).unwrap().as_duration(),
            Duration::from_secs(31)
        );
        assert_eq!(
            EveryDuration::try_from(Duration::from_secs(32)).unwrap().as_duration(),
            Duration::from_secs(32)
        );
        assert_eq!(
            EveryDuration::try_from(33).unwrap().as_duration(),
            Duration::from_secs(33)
        );
        assert!(EveryDuration::new(Duration::ZERO).is_err());
        assert!(EveryDuration::new(PROTOBUF_DURATION_MAX + Duration::from_nanos(1)).is_err());
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
    fn schedule_headers_cover_helpers_and_reserved_names() {
        let headers = ScheduleHeaders::new([("x-kind", "heartbeat")]).unwrap();
        let message_headers = MessageHeaders::new([("x-kind", "heartbeat")]).unwrap();
        let from_message_headers = ScheduleHeaders::try_from(message_headers).unwrap();

        assert!(!headers.is_empty());
        assert_eq!(headers.as_slice()[0].name().as_str(), "x-kind");
        assert_eq!(headers.clone().into_message_headers().as_slice().len(), 1);
        assert_eq!(MessageHeaders::from(from_message_headers).as_slice().len(), 1);
        assert!(ScheduleHeaders::new([("Nats-Schedule", "value")]).is_err());
        assert!(ScheduleHeaders::new([("bad name", "value")]).is_err());
        assert!(ScheduleHeaders::new([("x-kind", "bad\nvalue")]).is_err());
    }

    #[test]
    fn route_sampling_and_delivery_cover_conversions() {
        let route = DeliveryRoute::try_from("agent.run".to_string()).unwrap();
        let route_ref = DeliveryRoute::try_from("agent.reply").unwrap();
        let subject = SamplingSubject::try_from("agent.events".to_string()).unwrap();
        let subject_ref = SamplingSubject::try_from("agent.replay").unwrap();
        let ttl = TtlDuration::new(Duration::from_secs(60)).unwrap();
        let source = SamplingSource::latest_from_subject("agent.events").unwrap();
        let delivery = Delivery::NatsEvent {
            route: route.clone(),
            ttl: Some(ttl),
            source: Some(source.clone()),
        };

        assert_eq!(route.as_str(), "agent.run");
        assert_eq!(route.as_token().as_str(), "agent.run");
        assert_eq!(route_ref.as_str(), "agent.reply");
        assert_eq!(subject.as_str(), "agent.events");
        assert_eq!(subject_ref.as_str(), "agent.replay");
        assert_eq!(ttl.as_duration(), Duration::from_secs(60));
        assert_eq!(
            TtlDuration::from_secs(61).unwrap().as_duration(),
            Duration::from_secs(61)
        );
        assert_eq!(
            TtlDuration::try_from(Duration::from_secs(62)).unwrap().as_duration(),
            Duration::from_secs(62)
        );
        assert_eq!(
            TtlDuration::try_from(63).unwrap().as_duration(),
            Duration::from_secs(63)
        );
        assert!(TtlDuration::new(Duration::ZERO).is_err());
        assert!(TtlDuration::new(PROTOBUF_DURATION_MAX + Duration::from_nanos(1)).is_err());
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
                ttl: Some(_),
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
            Schedule::every(Duration::from_secs(30)).unwrap(),
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

        let message = ScheduleMessage {
            content: MessageContent::json("{}"),
            headers: ScheduleHeaders::new([("x-kind", "heartbeat")]).unwrap(),
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
            fn every_duration_accepts_any_positive_duration_and_round_trips(n in 1u64..=PROTOBUF_DURATION_MAX_SECONDS) {
                let duration = Duration::from_secs(n);
                let every = EveryDuration::new(duration).unwrap();
                prop_assert_eq!(every.as_duration(), duration);
            }

            #[test]
            fn ttl_duration_accepts_any_positive_duration_and_round_trips(n in 1u64..=PROTOBUF_DURATION_MAX_SECONDS) {
                let duration = Duration::from_secs(n);
                let ttl = TtlDuration::new(duration).unwrap();
                prop_assert_eq!(ttl.as_duration(), duration);
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

                let result = ScheduleHeaders::new([(name, value)]);
                let is_reserved_error = matches!(result, Err(ScheduleHeadersError::ReservedName { .. }));
                prop_assert!(is_reserved_error);
            }

            #[test]
            fn non_reserved_headers_construct_and_preserve_input(
                name in "x-[a-z]{1,12}",
                value in "[ -~]{0,16}",
            ) {
                let headers = ScheduleHeaders::new([(name.clone(), value.clone())]).unwrap();
                let slice = headers.as_slice();
                prop_assert_eq!(slice.len(), 1);
                prop_assert_eq!(slice[0].name().as_str(), name.as_str());
                prop_assert_eq!(slice[0].value().as_str(), value.as_str());
            }

            #[test]
            fn schedule_every_constructor_matches_value_object(n in 1u64..=PROTOBUF_DURATION_MAX_SECONDS) {
                let duration = Duration::from_secs(n);
                let schedule = Schedule::every(duration).unwrap();
                match schedule {
                    Schedule::Every { every } => prop_assert_eq!(every.as_duration(), duration),
                    other => prop_assert!(false, "expected Every, got {other:?}"),
                }
            }
        }
    }
}
