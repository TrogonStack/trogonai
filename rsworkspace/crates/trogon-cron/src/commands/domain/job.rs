use std::{num::NonZeroU64, str::FromStr};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use trogon_nats::DottedNatsToken;

use crate::error::JobSpecError;

use super::{
    JobDetails, JobEventDelivery, JobEventSamplingSource, JobEventSchedule, JobEventStatus, JobId, MessageContent,
    MessageEnvelope, MessageHeaders, MessageHeadersError,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Job {
    pub id: JobId,
    #[serde(default, rename = "state")]
    pub status: JobStatus,
    pub schedule: Schedule,
    pub delivery: Delivery,
    pub message: JobMessage,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Schedule {
    At {
        at: DateTime<Utc>,
    },
    Every {
        every_sec: EverySeconds,
    },
    Cron {
        expr: CronExpression,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timezone: Option<ScheduleTimezone>,
    },
}

impl Schedule {
    pub fn every(every_sec: u64) -> Result<Self, JobSpecError> {
        Ok(Self::Every {
            every_sec: EverySeconds::new(every_sec)?,
        })
    }

    pub fn cron(expr: impl Into<String>, timezone: Option<String>) -> Result<Self, JobSpecError> {
        Ok(Self::Cron {
            expr: CronExpression::new(expr)?,
            timezone: timezone.map(ScheduleTimezone::new).transpose()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EverySeconds(NonZeroU64);

impl EverySeconds {
    pub fn new(every_sec: u64) -> Result<Self, JobSpecError> {
        NonZeroU64::new(every_sec)
            .map(Self)
            .ok_or(JobSpecError::EverySecondsMustBePositive)
    }

    pub fn get(self) -> u64 {
        self.0.get()
    }
}

impl TryFrom<u64> for EverySeconds {
    type Error = JobSpecError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl Serialize for EverySeconds {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(self.get())
    }
}

impl<'de> Deserialize<'de> for EverySeconds {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let every_sec = u64::deserialize(deserializer)?;
        Self::new(every_sec).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronExpression(String);

impl CronExpression {
    pub fn new(expr: impl Into<String>) -> Result<Self, JobSpecError> {
        let expr = expr.into();
        cron::Schedule::from_str(&expr).map_err(|source| JobSpecError::InvalidCronExpression {
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
    type Error = JobSpecError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for CronExpression {
    type Error = JobSpecError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl Serialize for CronExpression {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CronExpression {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let expr = String::deserialize(deserializer)?;
        Self::new(expr).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleTimezone(String);

impl ScheduleTimezone {
    pub fn new(timezone: impl Into<String>) -> Result<Self, JobSpecError> {
        let timezone = timezone.into();
        let trimmed = timezone.trim();
        if trimmed.is_empty() || trimmed != timezone || trimmed.chars().any(|ch| ch.is_control() || ch.is_whitespace())
        {
            return Err(JobSpecError::InvalidTimezone { timezone });
        }

        Ok(Self(timezone))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl TryFrom<String> for ScheduleTimezone {
    type Error = JobSpecError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for ScheduleTimezone {
    type Error = JobSpecError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl Serialize for ScheduleTimezone {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ScheduleTimezone {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let timezone = String::deserialize(deserializer)?;
        Self::new(timezone).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobHeaders(MessageHeaders);

impl JobHeaders {
    pub fn new<I, N, V>(headers: I) -> Result<Self, JobSpecError>
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

    pub fn as_slice(&self) -> &[(String, String)] {
        self.0.as_slice()
    }

    pub fn into_message_headers(self) -> MessageHeaders {
        self.0
    }
}

fn message_headers_error(source: MessageHeadersError) -> JobSpecError {
    match source {
        MessageHeadersError::InvalidName { name } => JobSpecError::InvalidHeaderName { name },
        MessageHeadersError::InvalidValue { name } => JobSpecError::InvalidHeaderValue { name },
    }
}

impl TryFrom<MessageHeaders> for JobHeaders {
    type Error = JobSpecError;

    fn try_from(value: MessageHeaders) -> Result<Self, Self::Error> {
        validate_reserved_scheduler_headers(value.as_slice())?;
        Ok(Self(value))
    }
}

impl Serialize for JobHeaders {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for JobHeaders {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let headers = MessageHeaders::deserialize(deserializer)?;
        Self::try_from(headers).map_err(D::Error::custom)
    }
}

impl From<JobHeaders> for MessageHeaders {
    fn from(value: JobHeaders) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct JobMessage {
    pub content: MessageContent,
    #[serde(default, skip_serializing_if = "JobHeaders::is_empty")]
    pub headers: JobHeaders,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryRoute(DottedNatsToken);

impl DeliveryRoute {
    pub fn new(route: impl AsRef<str>) -> Result<Self, JobSpecError> {
        let route = route.as_ref();
        DottedNatsToken::new(route)
            .map(Self)
            .map_err(|source| JobSpecError::InvalidRoute {
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
    type Error = JobSpecError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for DeliveryRoute {
    type Error = JobSpecError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl Serialize for DeliveryRoute {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for DeliveryRoute {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let route = String::deserialize(deserializer)?;
        Self::new(route).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SamplingSubject(DottedNatsToken);

impl SamplingSubject {
    pub fn new(subject: impl AsRef<str>) -> Result<Self, JobSpecError> {
        let subject = subject.as_ref();
        DottedNatsToken::new(subject)
            .map(Self)
            .map_err(|source| JobSpecError::InvalidSamplingSource {
                subject: subject.to_string(),
                source,
            })
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl TryFrom<String> for SamplingSubject {
    type Error = JobSpecError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for SamplingSubject {
    type Error = JobSpecError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl Serialize for SamplingSubject {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SamplingSubject {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let subject = String::deserialize(deserializer)?;
        Self::new(subject).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TtlSeconds(NonZeroU64);

impl TtlSeconds {
    pub fn new(ttl_sec: u64) -> Result<Self, JobSpecError> {
        NonZeroU64::new(ttl_sec)
            .map(Self)
            .ok_or(JobSpecError::TtlMustBePositive)
    }

    pub fn get(self) -> u64 {
        self.0.get()
    }
}

impl TryFrom<u64> for TtlSeconds {
    type Error = JobSpecError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl Serialize for TtlSeconds {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(self.get())
    }
}

impl<'de> Deserialize<'de> for TtlSeconds {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let ttl_sec = u64::deserialize(deserializer)?;
        Self::new(ttl_sec).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SamplingSource {
    LatestFromSubject { subject: SamplingSubject },
}

impl SamplingSource {
    pub fn latest_from_subject(subject: impl AsRef<str>) -> Result<Self, JobSpecError> {
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Delivery {
    NatsEvent {
        route: DeliveryRoute,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ttl_sec: Option<TtlSeconds>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<SamplingSource>,
    },
}

impl Delivery {
    pub fn nats_event(route: impl AsRef<str>) -> Result<Self, JobSpecError> {
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

fn validate_reserved_scheduler_headers(headers: &[(String, String)]) -> Result<(), JobSpecError> {
    for (name, _) in headers {
        if RESERVED_SCHEDULE_HEADERS
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(name))
        {
            return Err(JobSpecError::ReservedHeaderName { name: name.clone() });
        }
    }

    Ok(())
}

impl From<JobStatus> for JobEventStatus {
    fn from(value: JobStatus) -> Self {
        match value {
            JobStatus::Enabled => Self::Enabled,
            JobStatus::Disabled => Self::Disabled,
        }
    }
}

impl From<JobEventStatus> for JobStatus {
    fn from(value: JobEventStatus) -> Self {
        match value {
            JobEventStatus::Enabled => Self::Enabled,
            JobEventStatus::Disabled => Self::Disabled,
        }
    }
}

impl From<Schedule> for JobEventSchedule {
    fn from(value: Schedule) -> Self {
        match value {
            Schedule::At { at } => Self::At { at },
            Schedule::Every { every_sec } => Self::Every {
                every_sec: every_sec.get(),
            },
            Schedule::Cron { expr, timezone } => Self::Cron {
                expr: expr.into_string(),
                timezone: timezone.map(ScheduleTimezone::into_string),
            },
        }
    }
}

impl From<&Schedule> for JobEventSchedule {
    fn from(value: &Schedule) -> Self {
        match value {
            Schedule::At { at } => Self::At { at: *at },
            Schedule::Every { every_sec } => Self::Every {
                every_sec: every_sec.get(),
            },
            Schedule::Cron { expr, timezone } => Self::Cron {
                expr: expr.as_str().to_string(),
                timezone: timezone.as_ref().map(|timezone| timezone.as_str().to_string()),
            },
        }
    }
}

impl From<SamplingSource> for JobEventSamplingSource {
    fn from(value: SamplingSource) -> Self {
        match value {
            SamplingSource::LatestFromSubject { subject } => Self::LatestFromSubject {
                subject: subject.as_str().to_string(),
            },
        }
    }
}

impl From<&SamplingSource> for JobEventSamplingSource {
    fn from(value: &SamplingSource) -> Self {
        match value {
            SamplingSource::LatestFromSubject { subject } => Self::LatestFromSubject {
                subject: subject.as_str().to_string(),
            },
        }
    }
}

impl From<Delivery> for JobEventDelivery {
    fn from(value: Delivery) -> Self {
        match value {
            Delivery::NatsEvent { route, ttl_sec, source } => Self::NatsEvent {
                route: route.as_str().to_string(),
                ttl_sec: ttl_sec.map(TtlSeconds::get),
                source: source.map(Into::into),
            },
        }
    }
}

impl From<&Delivery> for JobEventDelivery {
    fn from(value: &Delivery) -> Self {
        match value {
            Delivery::NatsEvent { route, ttl_sec, source } => Self::NatsEvent {
                route: route.as_str().to_string(),
                ttl_sec: ttl_sec.map(TtlSeconds::get),
                source: source.as_ref().map(Into::into),
            },
        }
    }
}

impl From<Job> for JobDetails {
    fn from(job: Job) -> Self {
        Self {
            status: job.status.into(),
            schedule: job.schedule.into(),
            delivery: job.delivery.into(),
            message: job.message.into(),
        }
    }
}

impl From<&Job> for JobDetails {
    fn from(job: &Job) -> Self {
        Self {
            status: job.status.into(),
            schedule: (&job.schedule).into(),
            delivery: (&job.delivery).into(),
            message: (&job.message).into(),
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
    use trogon_eventsourcing::Snapshot;

    fn job_id(id: &str) -> JobId {
        JobId::parse(id).unwrap()
    }

    fn route(value: &str) -> DeliveryRoute {
        DeliveryRoute::new(value).unwrap()
    }

    fn ttl(value: u64) -> TtlSeconds {
        TtlSeconds::new(value).unwrap()
    }

    fn source(value: &str) -> SamplingSource {
        SamplingSource::latest_from_subject(value).unwrap()
    }

    #[test]
    fn job_status_defaults_to_enabled() {
        let raw = r#"{
            "id": "heartbeat",
            "schedule": { "type": "every", "every_sec": 30 },
            "delivery": { "type": "nats_event", "route": "agent.run" },
            "message": {
                "content": "{\"kind\":\"heartbeat\"}"
            }
        }"#;

        let job: Job = serde_json::from_str(raw).unwrap();

        assert_eq!(job.status, JobStatus::Enabled);
    }

    #[test]
    fn job_round_trips() {
        let job = Job {
            id: job_id("compact"),
            status: JobStatus::Enabled,
            schedule: Schedule::cron("0 */5 * * * *", Some("UTC".to_string())).unwrap(),
            delivery: Delivery::NatsEvent {
                route: route("workflow.compact"),
                ttl_sec: Some(ttl(30)),
                source: Some(source("sensors.latest")),
            },
            message: JobMessage {
                content: MessageContent::from_static(r#"{"workflow":"compact"}"#),
                headers: JobHeaders::new([("owner", "ops")]).unwrap(),
            },
        };

        let json = serde_json::to_string(&job).unwrap();
        let decoded: Job = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded, job);
    }

    #[test]
    fn empty_headers_are_omitted() {
        let job = Job {
            id: job_id("compact"),
            status: JobStatus::Enabled,
            schedule: Schedule::every(30).unwrap(),
            delivery: Delivery::NatsEvent {
                route: route("agent.run"),
                ttl_sec: None,
                source: None,
            },
            message: JobMessage {
                content: MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
                headers: JobHeaders::default(),
            },
        };

        let json = serde_json::to_string(&job).unwrap();

        assert!(!json.contains("\"headers\""));
        assert!(json.contains("\"state\":\"enabled\""));
    }

    #[test]
    fn snapshot_round_trips() {
        let snapshot = Snapshot::new(
            9,
            Job {
                id: job_id("compact"),
                status: JobStatus::Enabled,
                schedule: Schedule::every(30).unwrap(),
                delivery: Delivery::NatsEvent {
                    route: route("agent.run"),
                    ttl_sec: None,
                    source: None,
                },
                message: JobMessage {
                    content: MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
                    headers: JobHeaders::default(),
                },
            },
        );

        let json = serde_json::to_string(&snapshot).unwrap();
        let decoded: Snapshot<Job> = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn invalid_route_is_rejected_during_deserialization() {
        let error = serde_json::from_value::<Job>(serde_json::json!({
            "id": "heartbeat",
            "schedule": { "type": "every", "every_sec": 30 },
            "delivery": { "type": "nats_event", "route": "agent.>" },
            "message": {
                "content": "{\"kind\":\"heartbeat\"}"
            }
        }))
        .unwrap_err();

        assert!(error.to_string().contains("route"));
    }

    #[test]
    fn invalid_sampling_source_is_rejected_during_deserialization() {
        let error = serde_json::from_value::<Job>(serde_json::json!({
            "id": "heartbeat",
            "schedule": { "type": "every", "every_sec": 30 },
            "delivery": {
                "type": "nats_event",
                "route": "agent.run",
                "source": {
                    "type": "latest_from_subject",
                    "subject": "jobs.>"
                }
            },
            "message": {
                "content": "{\"kind\":\"heartbeat\"}"
            }
        }))
        .unwrap_err();

        assert!(error.to_string().contains("sampling source"));
    }

    #[test]
    fn zero_ttl_is_rejected_during_deserialization() {
        let error = serde_json::from_value::<Job>(serde_json::json!({
            "id": "heartbeat",
            "schedule": { "type": "every", "every_sec": 30 },
            "delivery": {
                "type": "nats_event",
                "route": "agent.run",
                "ttl_sec": 0
            },
            "message": {
                "content": "{\"kind\":\"heartbeat\"}"
            }
        }))
        .unwrap_err();

        assert!(error.to_string().contains("ttl_sec"));
    }

    #[test]
    fn zero_every_seconds_is_rejected_during_deserialization() {
        let error = serde_json::from_value::<Job>(serde_json::json!({
            "id": "heartbeat",
            "schedule": { "type": "every", "every_sec": 0 },
            "delivery": { "type": "nats_event", "route": "agent.run" },
            "message": {
                "content": "{\"kind\":\"heartbeat\"}"
            }
        }))
        .unwrap_err();

        assert!(error.to_string().contains("every_sec"));
    }

    #[test]
    fn invalid_cron_expression_is_rejected_during_deserialization() {
        let error = serde_json::from_value::<Job>(serde_json::json!({
            "id": "heartbeat",
            "schedule": { "type": "cron", "expr": "not-a-cron" },
            "delivery": { "type": "nats_event", "route": "agent.run" },
            "message": {
                "content": "{\"kind\":\"heartbeat\"}"
            }
        }))
        .unwrap_err();

        assert!(error.to_string().contains("cron expression"));
    }

    #[test]
    fn invalid_timezone_is_rejected_during_deserialization() {
        let error = serde_json::from_value::<Job>(serde_json::json!({
            "id": "heartbeat",
            "schedule": {
                "type": "cron",
                "expr": "0 */5 * * * *",
                "timezone": " America/New_York "
            },
            "delivery": { "type": "nats_event", "route": "agent.run" },
            "message": {
                "content": "{\"kind\":\"heartbeat\"}"
            }
        }))
        .unwrap_err();

        assert!(error.to_string().contains("timezone"));
    }

    #[test]
    fn reserved_header_is_rejected_during_deserialization() {
        let error = serde_json::from_value::<Job>(serde_json::json!({
            "id": "heartbeat",
            "schedule": { "type": "every", "every_sec": 30 },
            "delivery": { "type": "nats_event", "route": "agent.run" },
            "message": {
                "headers": [["Nats-Schedule-Target", "cron.fire.evil.target"]],
                "content": "{\"kind\":\"heartbeat\"}"
            }
        }))
        .unwrap_err();

        assert!(error.to_string().contains("reserved"));
    }

    #[test]
    fn invalid_header_value_is_rejected_during_deserialization() {
        let error = serde_json::from_value::<Job>(serde_json::json!({
            "id": "heartbeat",
            "schedule": { "type": "every", "every_sec": 30 },
            "delivery": { "type": "nats_event", "route": "agent.run" },
            "message": {
                "headers": [["x-kind", "bad\nvalue"]],
                "content": "{\"kind\":\"heartbeat\"}"
            }
        }))
        .unwrap_err();

        assert!(error.to_string().contains("invalid value"));
    }

    #[test]
    fn job_status_helpers_work() {
        assert!(JobStatus::Enabled.is_enabled());
        assert_eq!(JobStatus::Enabled.as_str(), "enabled");
        assert!(!JobStatus::Disabled.is_enabled());
        assert_eq!(JobStatus::Disabled.as_str(), "disabled");
    }

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        const RESERVED_HEADER_NAMES: [&str; 5] = [
            "Nats-Schedule",
            "Nats-Schedule-Source",
            "Nats-Schedule-Target",
            "Nats-Schedule-Time-Zone",
            "Nats-Schedule-TTL",
        ];

        proptest! {
            #[test]
            fn every_seconds_accepts_any_positive_and_round_trips(n in 1u64..) {
                let every = EverySeconds::new(n).unwrap();
                prop_assert_eq!(every.get(), n);
            }

            #[test]
            fn ttl_seconds_accepts_any_positive_and_round_trips(n in 1u64..) {
                let ttl = TtlSeconds::new(n).unwrap();
                prop_assert_eq!(ttl.get(), n);
            }

            #[test]
            fn delivery_route_accepts_any_well_formed_dotted_token(
                s in "[a-z][a-z0-9_-]{0,15}(\\.[a-z][a-z0-9_-]{0,15}){0,5}",
            ) {
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
                shape in 0u8..3,
            ) {
                let s = match shape {
                    0 => format!(".{core}"),
                    1 => format!("{core}."),
                    _ => format!("{core}..tail"),
                };
                prop_assert!(DeliveryRoute::new(&s).is_err());
            }

            #[test]
            fn sampling_subject_accepts_any_well_formed_dotted_token(
                s in "[a-z][a-z0-9_-]{0,15}(\\.[a-z][a-z0-9_-]{0,15}){0,5}",
            ) {
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
            fn schedule_timezone_accepts_any_non_whitespace_non_control_string(
                s in "[A-Za-z][A-Za-z0-9/_+-]{0,31}",
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
                name_template in proptest::sample::select(&RESERVED_HEADER_NAMES[..]),
                upper_mask in any::<u64>(),
                value in "[ -~]{0,16}",
            ) {
                let name: String = name_template
                    .chars()
                    .enumerate()
                    .map(|(i, ch)| {
                        if (upper_mask >> (i % 64)) & 1 == 1 {
                            ch.to_ascii_uppercase()
                        } else {
                            ch.to_ascii_lowercase()
                        }
                    })
                    .collect();

                let result = JobHeaders::new([(name, value)]);
                let is_reserved_error = matches!(result, Err(JobSpecError::ReservedHeaderName { .. }));
                prop_assert!(is_reserved_error);
            }

            #[test]
            fn non_reserved_headers_pass_the_reserved_check(
                name in "x-[a-z]{1,12}",
                value in "[ -~]{0,16}",
            ) {
                let headers = JobHeaders::new([(name, value)]).unwrap();
                prop_assert!(!headers.is_empty());
            }

            #[test]
            fn schedule_every_constructor_matches_value_object(n in 1u64..) {
                let schedule = Schedule::every(n).unwrap();
                match schedule {
                    Schedule::Every { every_sec } => prop_assert_eq!(every_sec.get(), n),
                    other => prop_assert!(false, "expected Every, got {other:?}"),
                }
            }
        }
    }
}
