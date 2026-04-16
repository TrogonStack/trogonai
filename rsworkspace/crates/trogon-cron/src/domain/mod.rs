mod job_id;
mod spec;

use std::str::FromStr;

use async_nats::{HeaderMap, HeaderValue};
use bytes::Bytes;
use trogon_nats::{DottedNatsToken, NatsToken};

use crate::{
    error::{CronError, JobSpecError},
    kv::{FIRE_SUBJECT_PREFIX, SCHEDULE_SUBJECT_PREFIX},
};

pub use job_id::{JobId, JobIdError};
pub use spec::{DeliverySpec, JobEnabledState, JobSpec, SamplingSource, ScheduleSpec};

use std::collections::BTreeMap;

const NATS_SCHEDULE: &str = "Nats-Schedule";
const NATS_SCHEDULE_SOURCE: &str = "Nats-Schedule-Source";
const NATS_SCHEDULE_TARGET: &str = "Nats-Schedule-Target";
const NATS_SCHEDULE_TIME_ZONE: &str = "Nats-Schedule-Time-Zone";
const NATS_SCHEDULE_TTL: &str = "Nats-Schedule-TTL";
const RESERVED_SCHEDULE_HEADERS: [&str; 5] = [
    NATS_SCHEDULE,
    NATS_SCHEDULE_SOURCE,
    NATS_SCHEDULE_TARGET,
    NATS_SCHEDULE_TIME_ZONE,
    NATS_SCHEDULE_TTL,
];

#[derive(Debug, Clone)]
pub struct ResolvedJobSpec {
    spec: JobSpec,
    job_id: NatsToken,
    route: DottedNatsToken,
    schedule_subject: String,
    target_subject: String,
    schedule_expression: String,
    timezone: Option<String>,
    ttl_sec: Option<u64>,
    source_subject: Option<String>,
    headers: BTreeMap<String, String>,
    body: Bytes,
}

struct ValidatedJobSpecParts {
    job_id: NatsToken,
    route: DottedNatsToken,
    schedule_expression: String,
    timezone: Option<String>,
    ttl_sec: Option<u64>,
    source_subject: Option<String>,
    headers: BTreeMap<String, String>,
}

impl TryFrom<&JobSpec> for ResolvedJobSpec {
    type Error = CronError;

    fn try_from(spec: &JobSpec) -> Result<Self, Self::Error> {
        let ValidatedJobSpecParts {
            job_id,
            route,
            schedule_expression,
            timezone,
            ttl_sec,
            source_subject,
            headers,
        } = validated_job_spec_parts(spec)?;

        let schedule_subject = format!("{SCHEDULE_SUBJECT_PREFIX}{}", job_id.as_str());
        let target_subject = format!(
            "{FIRE_SUBJECT_PREFIX}{}.{}",
            route.as_str(),
            job_id.as_str()
        );
        let body = if source_subject.is_some() {
            Bytes::from_static(br#"{}"#)
        } else {
            serde_json::to_vec(&spec.payload)?.into()
        };

        Ok(Self {
            spec: spec.clone(),
            job_id,
            route,
            schedule_subject,
            target_subject,
            schedule_expression,
            timezone,
            ttl_sec,
            source_subject,
            headers,
            body,
        })
    }
}

fn validated_job_spec_parts(spec: &JobSpec) -> Result<ValidatedJobSpecParts, CronError> {
    let job_id = parse_job_id(&spec.id);
    let schedule_expression = schedule_expression(&spec.schedule)?;
    let timezone = match &spec.schedule {
        ScheduleSpec::Cron { timezone, .. } => validate_timezone(timezone.clone())?,
        ScheduleSpec::At { .. } | ScheduleSpec::Every { .. } => None,
    };
    let (route, headers, ttl_sec, source_subject) = match &spec.delivery {
        DeliverySpec::NatsEvent {
            route,
            headers,
            ttl_sec,
            source,
        } => (
            validate_route(route)?,
            validate_headers(headers)?,
            validate_ttl(*ttl_sec)?,
            validate_source(source)?,
        ),
    };

    Ok(ValidatedJobSpecParts {
        job_id,
        route,
        schedule_expression,
        timezone,
        ttl_sec,
        source_subject,
        headers,
    })
}

fn parse_job_id(id: &JobId) -> NatsToken {
    id.as_token().clone()
}

impl ResolvedJobSpec {
    pub fn spec(&self) -> &JobSpec {
        &self.spec
    }

    pub fn id(&self) -> &str {
        self.job_id.as_str()
    }

    pub fn enabled(&self) -> bool {
        self.spec.state.is_enabled()
    }

    pub fn route(&self) -> &str {
        self.route.as_str()
    }

    pub fn schedule_subject(&self) -> &str {
        &self.schedule_subject
    }

    pub fn target_subject(&self) -> &str {
        &self.target_subject
    }

    pub fn source_subject(&self) -> Option<&str> {
        self.source_subject.as_deref()
    }

    pub fn schedule_body(&self) -> Bytes {
        self.body.clone()
    }

    pub fn schedule_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            NATS_SCHEDULE,
            HeaderValue::from(self.schedule_expression.as_str()),
        );
        headers.insert(
            NATS_SCHEDULE_TARGET,
            HeaderValue::from(self.target_subject.as_str()),
        );

        if let Some(timezone) = &self.timezone {
            headers.insert(
                NATS_SCHEDULE_TIME_ZONE,
                HeaderValue::from(timezone.as_str()),
            );
        }
        if let Some(ttl_sec) = self.ttl_sec {
            headers.insert(NATS_SCHEDULE_TTL, HeaderValue::from(format!("{ttl_sec}s")));
        }
        if let Some(source_subject) = &self.source_subject {
            headers.insert(
                NATS_SCHEDULE_SOURCE,
                HeaderValue::from(source_subject.as_str()),
            );
        }
        for (name, value) in &self.headers {
            headers.insert(name.as_str(), value.as_str());
        }

        headers
    }
}

fn schedule_expression(schedule: &ScheduleSpec) -> Result<String, CronError> {
    match schedule {
        ScheduleSpec::At { at } => Ok(format!("@at {}", at.to_rfc3339())),
        ScheduleSpec::Every { every_sec } => {
            if *every_sec == 0 {
                return Err(CronError::invalid_job_spec(
                    JobSpecError::EverySecondsMustBePositive,
                ));
            }
            Ok(format!("@every {every_sec}s"))
        }
        ScheduleSpec::Cron { expr, .. } => {
            cron::Schedule::from_str(expr).map_err(|source| {
                CronError::invalid_job_spec(JobSpecError::InvalidCronExpression {
                    expr: expr.clone(),
                    source: Box::new(source),
                })
            })?;
            Ok(expr.clone())
        }
    }
}

fn validate_route(route: &str) -> Result<DottedNatsToken, CronError> {
    DottedNatsToken::new(route).map_err(|source| {
        CronError::invalid_job_spec(JobSpecError::InvalidRoute {
            route: route.to_string(),
            source,
        })
    })
}

fn validate_source(source: &Option<SamplingSource>) -> Result<Option<String>, CronError> {
    match source {
        None => Ok(None),
        Some(SamplingSource::LatestFromSubject { subject }) => {
            DottedNatsToken::new(subject).map_err(|source| {
                CronError::invalid_job_spec(JobSpecError::InvalidSamplingSource {
                    subject: subject.clone(),
                    source,
                })
            })?;
            Ok(Some(subject.clone()))
        }
    }
}

fn validate_ttl(ttl_sec: Option<u64>) -> Result<Option<u64>, CronError> {
    match ttl_sec {
        Some(0) => Err(CronError::invalid_job_spec(JobSpecError::TtlMustBePositive)),
        Some(ttl_sec) => Ok(Some(ttl_sec)),
        None => Ok(None),
    }
}

fn validate_timezone(timezone: Option<String>) -> Result<Option<String>, CronError> {
    let Some(timezone) = timezone else {
        return Ok(None);
    };
    let trimmed = timezone.trim();
    if trimmed.is_empty()
        || trimmed != timezone
        || trimmed
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(CronError::invalid_job_spec(JobSpecError::InvalidTimezone {
            timezone,
        }));
    }
    Ok(Some(timezone))
}

fn validate_headers(
    headers: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, CronError> {
    for (name, value) in headers {
        if name.trim().is_empty()
            || name.contains(':')
            || name.chars().any(|ch| ch.is_control() || ch.is_whitespace())
        {
            return Err(CronError::invalid_job_spec(
                JobSpecError::InvalidHeaderName { name: name.clone() },
            ));
        }
        if RESERVED_SCHEDULE_HEADERS
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(name))
        {
            return Err(CronError::invalid_job_spec(
                JobSpecError::ReservedHeaderName { name: name.clone() },
            ));
        }
        if value
            .chars()
            .any(|ch| ch == '\r' || ch == '\n' || ch == '\0')
        {
            return Err(CronError::invalid_job_spec(
                JobSpecError::InvalidHeaderValue { name: name.clone() },
            ));
        }
    }

    Ok(headers.clone())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::{JobEnabledState, JobId, JobSpec};

    fn job_id(id: &str) -> JobId {
        JobId::parse(id).unwrap()
    }

    fn base_job() -> JobSpec {
        JobSpec {
            id: job_id("heartbeat"),
            state: JobEnabledState::Enabled,
            schedule: ScheduleSpec::Every { every_sec: 30 },
            delivery: DeliverySpec::NatsEvent {
                route: "agent.run".to_string(),
                headers: BTreeMap::from([("x-kind".to_string(), "heartbeat".to_string())]),
                ttl_sec: Some(15),
                source: None,
            },
            payload: serde_json::json!({"kind": "heartbeat"}),
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn resolved_job_derives_subjects_and_headers() {
        let resolved = ResolvedJobSpec::try_from(&base_job()).unwrap();

        assert_eq!(resolved.schedule_subject(), "cron.schedules.heartbeat");
        assert_eq!(resolved.target_subject(), "cron.fire.agent.run.heartbeat");
        assert_eq!(
            resolved
                .schedule_headers()
                .get(NATS_SCHEDULE)
                .map(|value| value.as_str()),
            Some("@every 30s")
        );
        assert_eq!(
            resolved
                .schedule_headers()
                .get(NATS_SCHEDULE_TTL)
                .map(|value| value.as_str()),
            Some("15s")
        );
    }

    #[test]
    fn at_schedule_maps_to_rfc3339() {
        let mut job = base_job();
        job.schedule = ScheduleSpec::At {
            at: Utc.with_ymd_and_hms(2026, 4, 11, 12, 0, 0).unwrap(),
        };

        let resolved = ResolvedJobSpec::try_from(&job).unwrap();

        assert_eq!(
            resolved
                .schedule_headers()
                .get(NATS_SCHEDULE)
                .map(|value| value.as_str()),
            Some("@at 2026-04-11T12:00:00+00:00")
        );
    }

    #[test]
    fn source_uses_placeholder_body() {
        let mut job = base_job();
        job.delivery = DeliverySpec::NatsEvent {
            route: "agent.run".to_string(),
            headers: BTreeMap::new(),
            ttl_sec: None,
            source: Some(SamplingSource::LatestFromSubject {
                subject: "sensors.latest".to_string(),
            }),
        };

        let resolved = ResolvedJobSpec::try_from(&job).unwrap();

        assert_eq!(resolved.schedule_body(), Bytes::from_static(br#"{}"#));
        assert_eq!(
            resolved
                .schedule_headers()
                .get(NATS_SCHEDULE_SOURCE)
                .map(|value| value.as_str()),
            Some("sensors.latest")
        );
    }

    #[test]
    fn invalid_route_is_rejected() {
        let mut job = base_job();
        job.delivery = DeliverySpec::NatsEvent {
            route: "agent.>".to_string(),
            headers: BTreeMap::new(),
            ttl_sec: None,
            source: None,
        };

        let error = ResolvedJobSpec::try_from(&job).unwrap_err();
        assert!(error.to_string().contains("route"));
    }

    #[test]
    fn reserved_scheduler_headers_are_rejected() {
        let mut job = base_job();
        job.delivery = DeliverySpec::NatsEvent {
            route: "agent.run".to_string(),
            headers: BTreeMap::from([(
                "Nats-Schedule-Target".to_string(),
                "cron.fire.evil.target".to_string(),
            )]),
            ttl_sec: None,
            source: None,
        };

        let error = ResolvedJobSpec::try_from(&job).unwrap_err();
        assert!(error.to_string().contains("reserved"));
    }

    #[test]
    fn reserved_scheduler_headers_are_rejected_case_insensitively() {
        let mut job = base_job();
        job.delivery = DeliverySpec::NatsEvent {
            route: "agent.run".to_string(),
            headers: BTreeMap::from([("nats-schedule".to_string(), "@every 1s".to_string())]),
            ttl_sec: None,
            source: None,
        };

        let error = ResolvedJobSpec::try_from(&job).unwrap_err();
        assert!(error.to_string().contains("reserved"));
    }

    #[test]
    fn resolved_job_accepts_valid_job() {
        ResolvedJobSpec::try_from(&base_job()).unwrap();
    }

    #[test]
    fn resolved_job_accessors_expose_validated_values() {
        let mut job = base_job();
        job.state = JobEnabledState::Disabled;

        let resolved = ResolvedJobSpec::try_from(&job).unwrap();

        assert_eq!(resolved.spec().id.as_str(), "heartbeat");
        assert_eq!(resolved.id(), "heartbeat");
        assert!(!resolved.enabled());
        assert_eq!(resolved.route(), "agent.run");
        assert_eq!(
            resolved.schedule_body(),
            Bytes::from_static(br#"{"kind":"heartbeat"}"#)
        );
        assert_eq!(resolved.source_subject(), None);
    }

    #[test]
    fn zero_every_seconds_is_rejected() {
        let mut job = base_job();
        job.schedule = ScheduleSpec::Every { every_sec: 0 };

        let error = ResolvedJobSpec::try_from(&job).unwrap_err();
        assert!(error.to_string().contains("every_sec"));
    }

    #[test]
    fn invalid_cron_expression_is_rejected() {
        let mut job = base_job();
        job.schedule = ScheduleSpec::Cron {
            expr: "not-a-cron".to_string(),
            timezone: None,
        };

        let error = ResolvedJobSpec::try_from(&job).unwrap_err();
        assert!(error.to_string().contains("cron expression"));
    }

    #[test]
    fn invalid_timezone_is_rejected() {
        let mut job = base_job();
        job.schedule = ScheduleSpec::Cron {
            expr: "0 * * * * *".to_string(),
            timezone: Some(" America/New_York ".to_string()),
        };

        let error = ResolvedJobSpec::try_from(&job).unwrap_err();
        assert!(error.to_string().contains("timezone"));
    }

    #[test]
    fn zero_ttl_is_rejected() {
        let mut job = base_job();
        job.delivery = DeliverySpec::NatsEvent {
            route: "agent.run".to_string(),
            headers: BTreeMap::new(),
            ttl_sec: Some(0),
            source: None,
        };

        let error = ResolvedJobSpec::try_from(&job).unwrap_err();
        assert!(error.to_string().contains("ttl_sec"));
    }

    #[test]
    fn invalid_sampling_source_is_rejected() {
        let mut job = base_job();
        job.delivery = DeliverySpec::NatsEvent {
            route: "agent.run".to_string(),
            headers: BTreeMap::new(),
            ttl_sec: None,
            source: Some(SamplingSource::LatestFromSubject {
                subject: "sensors.>".to_string(),
            }),
        };

        let error = ResolvedJobSpec::try_from(&job).unwrap_err();
        assert!(error.to_string().contains("sampling source"));
    }

    #[test]
    fn invalid_header_name_is_rejected() {
        let mut job = base_job();
        job.delivery = DeliverySpec::NatsEvent {
            route: "agent.run".to_string(),
            headers: BTreeMap::from([("bad:name".to_string(), "value".to_string())]),
            ttl_sec: None,
            source: None,
        };

        let error = ResolvedJobSpec::try_from(&job).unwrap_err();
        assert!(error.to_string().contains("header name"));
    }

    #[test]
    fn invalid_header_value_is_rejected() {
        let mut job = base_job();
        job.delivery = DeliverySpec::NatsEvent {
            route: "agent.run".to_string(),
            headers: BTreeMap::from([("x-kind".to_string(), "bad\nvalue".to_string())]),
            ttl_sec: None,
            source: None,
        };

        let error = ResolvedJobSpec::try_from(&job).unwrap_err();
        assert!(error.to_string().contains("invalid value"));
    }
}
