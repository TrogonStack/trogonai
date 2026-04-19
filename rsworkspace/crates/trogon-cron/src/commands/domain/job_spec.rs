use std::num::NonZeroU64;

use crate::{
    error::{CronError, JobSpecError},
    events::{
        JobDetails, JobEventDelivery, JobEventSamplingSource, JobEventSchedule, JobEventState,
        MessageContent, MessageHeaders,
    },
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use trogon_nats::DottedNatsToken;

use super::JobId;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobSpec {
    pub id: JobId,
    #[serde(default)]
    pub state: JobEnabledState,
    pub schedule: ScheduleSpec,
    pub delivery: DeliverySpec,
    pub content: MessageContent,
    #[serde(default, skip_serializing_if = "MessageHeaders::is_empty")]
    pub headers: MessageHeaders,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum JobEnabledState {
    #[default]
    Enabled,
    Disabled,
}

impl JobEnabledState {
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
pub enum ScheduleSpec {
    At {
        at: DateTime<Utc>,
    },
    Every {
        every_sec: u64,
    },
    Cron {
        expr: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timezone: Option<String>,
    },
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
        DottedNatsToken::new(subject).map(Self).map_err(|source| {
            JobSpecError::InvalidSamplingSource {
                subject: subject.to_string(),
                source,
            }
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
pub enum DeliverySpec {
    NatsEvent {
        route: DeliveryRoute,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ttl_sec: Option<TtlSeconds>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<SamplingSource>,
    },
}

impl DeliverySpec {
    pub fn nats_event(route: impl AsRef<str>) -> Result<Self, JobSpecError> {
        Ok(Self::NatsEvent {
            route: DeliveryRoute::new(route)?,
            ttl_sec: None,
            source: None,
        })
    }
}

impl From<JobEnabledState> for JobEventState {
    fn from(value: JobEnabledState) -> Self {
        match value {
            JobEnabledState::Enabled => Self::Enabled,
            JobEnabledState::Disabled => Self::Disabled,
        }
    }
}

impl From<JobEventState> for JobEnabledState {
    fn from(value: JobEventState) -> Self {
        match value {
            JobEventState::Enabled => Self::Enabled,
            JobEventState::Disabled => Self::Disabled,
        }
    }
}

impl From<ScheduleSpec> for JobEventSchedule {
    fn from(value: ScheduleSpec) -> Self {
        match value {
            ScheduleSpec::At { at } => Self::At { at },
            ScheduleSpec::Every { every_sec } => Self::Every { every_sec },
            ScheduleSpec::Cron { expr, timezone } => Self::Cron { expr, timezone },
        }
    }
}

impl From<&ScheduleSpec> for JobEventSchedule {
    fn from(value: &ScheduleSpec) -> Self {
        match value {
            ScheduleSpec::At { at } => Self::At { at: *at },
            ScheduleSpec::Every { every_sec } => Self::Every {
                every_sec: *every_sec,
            },
            ScheduleSpec::Cron { expr, timezone } => Self::Cron {
                expr: expr.clone(),
                timezone: timezone.clone(),
            },
        }
    }
}

impl From<JobEventSchedule> for ScheduleSpec {
    fn from(value: JobEventSchedule) -> Self {
        match value {
            JobEventSchedule::At { at } => Self::At { at },
            JobEventSchedule::Every { every_sec } => Self::Every { every_sec },
            JobEventSchedule::Cron { expr, timezone } => Self::Cron { expr, timezone },
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

impl TryFrom<JobEventSamplingSource> for SamplingSource {
    type Error = JobSpecError;

    fn try_from(value: JobEventSamplingSource) -> Result<Self, Self::Error> {
        match value {
            JobEventSamplingSource::LatestFromSubject { subject } => {
                Self::latest_from_subject(subject)
            }
        }
    }
}

impl From<DeliverySpec> for JobEventDelivery {
    fn from(value: DeliverySpec) -> Self {
        match value {
            DeliverySpec::NatsEvent {
                route,
                ttl_sec,
                source,
            } => Self::NatsEvent {
                route: route.as_str().to_string(),
                ttl_sec: ttl_sec.map(TtlSeconds::get),
                source: source.map(Into::into),
            },
        }
    }
}

impl From<&DeliverySpec> for JobEventDelivery {
    fn from(value: &DeliverySpec) -> Self {
        match value {
            DeliverySpec::NatsEvent {
                route,
                ttl_sec,
                source,
            } => Self::NatsEvent {
                route: route.as_str().to_string(),
                ttl_sec: ttl_sec.map(TtlSeconds::get),
                source: source.as_ref().map(Into::into),
            },
        }
    }
}

impl TryFrom<JobEventDelivery> for DeliverySpec {
    type Error = JobSpecError;

    fn try_from(value: JobEventDelivery) -> Result<Self, Self::Error> {
        match value {
            JobEventDelivery::NatsEvent {
                route,
                ttl_sec,
                source,
            } => Ok(Self::NatsEvent {
                route: DeliveryRoute::new(route)?,
                ttl_sec: ttl_sec.map(TtlSeconds::new).transpose()?,
                source: source.map(TryInto::try_into).transpose()?,
            }),
        }
    }
}

impl JobDetails {
    pub fn try_into_job_spec(self, id: JobId) -> Result<JobSpec, CronError> {
        Ok(JobSpec {
            id,
            state: self.state.into(),
            schedule: self.schedule.into(),
            delivery: self
                .delivery
                .try_into()
                .map_err(CronError::invalid_job_spec)?,
            content: self.content,
            headers: self.headers,
        })
    }
}

impl From<JobSpec> for JobDetails {
    fn from(spec: JobSpec) -> Self {
        Self {
            state: spec.state.into(),
            schedule: spec.schedule.into(),
            delivery: spec.delivery.into(),
            content: spec.content,
            headers: spec.headers,
        }
    }
}

impl From<&JobSpec> for JobDetails {
    fn from(spec: &JobSpec) -> Self {
        Self {
            state: spec.state.into(),
            schedule: (&spec.schedule).into(),
            delivery: (&spec.delivery).into(),
            content: spec.content.clone(),
            headers: spec.headers.clone(),
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
    fn job_spec_state_defaults_to_enabled() {
        let raw = r#"{
            "id": "heartbeat",
            "schedule": { "type": "every", "every_sec": 30 },
            "delivery": { "type": "nats_event", "route": "agent.run" },
            "content": "eyJraW5kIjoiaGVhcnRiZWF0In0="
        }"#;

        let job: JobSpec = serde_json::from_str(raw).unwrap();

        assert_eq!(job.state, JobEnabledState::Enabled);
    }

    #[test]
    fn job_spec_round_trips() {
        let job = JobSpec {
            id: job_id("compact"),
            state: JobEnabledState::Enabled,
            schedule: ScheduleSpec::Cron {
                expr: "0 */5 * * * *".to_string(),
                timezone: Some("UTC".to_string()),
            },
            delivery: DeliverySpec::NatsEvent {
                route: route("workflow.compact"),
                ttl_sec: Some(ttl(30)),
                source: Some(source("sensors.latest")),
            },
            content: MessageContent::from_static(br#"{"workflow":"compact"}"#),
            headers: MessageHeaders::new([("owner", "ops")]).unwrap(),
        };

        let json = serde_json::to_string(&job).unwrap();
        let decoded: JobSpec = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded, job);
    }

    #[test]
    fn empty_headers_are_omitted() {
        let job = JobSpec {
            id: job_id("compact"),
            state: JobEnabledState::Enabled,
            schedule: ScheduleSpec::Every { every_sec: 30 },
            delivery: DeliverySpec::NatsEvent {
                route: route("agent.run"),
                ttl_sec: None,
                source: None,
            },
            content: MessageContent::from_static(br#"{"kind":"heartbeat"}"#),
            headers: MessageHeaders::default(),
        };

        let json = serde_json::to_string(&job).unwrap();

        assert!(!json.contains("\"headers\""));
        assert!(json.contains("\"state\":\"enabled\""));
    }

    #[test]
    fn snapshot_round_trips() {
        let snapshot = Snapshot::new(
            9,
            JobSpec {
                id: job_id("compact"),
                state: JobEnabledState::Enabled,
                schedule: ScheduleSpec::Every { every_sec: 30 },
                delivery: DeliverySpec::NatsEvent {
                    route: route("agent.run"),
                    ttl_sec: None,
                    source: None,
                },
                content: MessageContent::from_static(br#"{"kind":"heartbeat"}"#),
                headers: MessageHeaders::default(),
            },
        );

        let json = serde_json::to_string(&snapshot).unwrap();
        let decoded: Snapshot<JobSpec> = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn job_details_round_trip_through_domain_conversion() {
        let details = JobDetails {
            state: JobEventState::Enabled,
            schedule: JobEventSchedule::Every { every_sec: 30 },
            delivery: JobEventDelivery::NatsEvent {
                route: "agent.run".to_string(),
                ttl_sec: Some(15),
                source: Some(JobEventSamplingSource::LatestFromSubject {
                    subject: "jobs.latest".to_string(),
                }),
            },
            content: MessageContent::from_static(br#"{"kind":"heartbeat"}"#),
            headers: MessageHeaders::new([("x-kind", "heartbeat")]).unwrap(),
        };

        let job = details
            .clone()
            .try_into_job_spec(job_id("heartbeat"))
            .unwrap();

        assert_eq!(job.id.as_str(), "heartbeat");
        assert_eq!(JobDetails::from(&job), details);
    }

    #[test]
    fn invalid_route_is_rejected_during_deserialization() {
        let error = serde_json::from_value::<JobSpec>(serde_json::json!({
            "id": "heartbeat",
            "schedule": { "type": "every", "every_sec": 30 },
            "delivery": { "type": "nats_event", "route": "agent.>" },
            "content": "eyJraW5kIjoiaGVhcnRiZWF0In0="
        }))
        .unwrap_err();

        assert!(error.to_string().contains("route"));
    }

    #[test]
    fn invalid_sampling_source_is_rejected_during_deserialization() {
        let error = serde_json::from_value::<JobSpec>(serde_json::json!({
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
            "content": "eyJraW5kIjoiaGVhcnRiZWF0In0="
        }))
        .unwrap_err();

        assert!(error.to_string().contains("sampling source"));
    }

    #[test]
    fn zero_ttl_is_rejected_during_deserialization() {
        let error = serde_json::from_value::<JobSpec>(serde_json::json!({
            "id": "heartbeat",
            "schedule": { "type": "every", "every_sec": 30 },
            "delivery": {
                "type": "nats_event",
                "route": "agent.run",
                "ttl_sec": 0
            },
            "content": "eyJraW5kIjoiaGVhcnRiZWF0In0="
        }))
        .unwrap_err();

        assert!(error.to_string().contains("ttl_sec"));
    }

    #[test]
    fn reserved_header_is_allowed_in_generic_job_spec_deserialization() {
        let spec = serde_json::from_value::<JobSpec>(serde_json::json!({
            "id": "heartbeat",
            "schedule": { "type": "every", "every_sec": 30 },
            "delivery": { "type": "nats_event", "route": "agent.run" },
            "headers": [["Nats-Schedule-Target", "cron.fire.evil.target"]],
            "content": "eyJraW5kIjoiaGVhcnRiZWF0In0="
        }))
        .unwrap();

        let headers = spec.headers;
        assert_eq!(
            headers.as_slice(),
            [(
                "Nats-Schedule-Target".to_string(),
                "cron.fire.evil.target".to_string(),
            )]
        );
    }

    #[test]
    fn invalid_header_value_is_rejected_during_deserialization() {
        let error = serde_json::from_value::<JobSpec>(serde_json::json!({
            "id": "heartbeat",
            "schedule": { "type": "every", "every_sec": 30 },
            "delivery": { "type": "nats_event", "route": "agent.run" },
            "headers": [["x-kind", "bad\nvalue"]],
            "content": "eyJraW5kIjoiaGVhcnRiZWF0In0="
        }))
        .unwrap_err();

        assert!(error.to_string().contains("invalid value"));
    }

    #[test]
    fn invalid_event_delivery_is_rejected_when_hydrating_domain_spec() {
        let error = JobDetails {
            state: JobEventState::Enabled,
            schedule: JobEventSchedule::Every { every_sec: 30 },
            delivery: JobEventDelivery::NatsEvent {
                route: "agent.run".to_string(),
                ttl_sec: Some(0),
                source: None,
            },
            content: MessageContent::from_static(br#"{"kind":"heartbeat"}"#),
            headers: MessageHeaders::default(),
        }
        .try_into_job_spec(job_id("heartbeat"))
        .unwrap_err();

        assert!(error.to_string().contains("ttl_sec"));
    }

    #[test]
    fn job_enabled_state_helpers_work() {
        assert!(JobEnabledState::Enabled.is_enabled());
        assert_eq!(JobEnabledState::Enabled.as_str(), "enabled");
        assert!(!JobEnabledState::Disabled.is_enabled());
        assert_eq!(JobEnabledState::Disabled.as_str(), "disabled");
    }
}
