use std::collections::BTreeMap;
use std::str::FromStr;

use async_nats::{HeaderMap, HeaderValue};
use bytes::Bytes;
use trogon_nats::{DottedNatsToken, NatsToken};

use super::{DeliverySpec, JobId, JobSpec, ScheduleSpec};
use crate::{
    error::{CronError, JobSpecError},
    kv::{FIRE_SUBJECT_PREFIX, SCHEDULE_SUBJECT_PREFIX},
};

const NATS_SCHEDULE: &str = "Nats-Schedule";
const NATS_SCHEDULE_SOURCE: &str = "Nats-Schedule-Source";
const NATS_SCHEDULE_TARGET: &str = "Nats-Schedule-Target";
const NATS_SCHEDULE_TIME_ZONE: &str = "Nats-Schedule-Time-Zone";
const NATS_SCHEDULE_TTL: &str = "Nats-Schedule-TTL";
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

struct ResolvedJobSpecParts {
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
        let ResolvedJobSpecParts {
            job_id,
            route,
            schedule_expression,
            timezone,
            ttl_sec,
            source_subject,
            headers,
        } = resolved_job_spec_parts(spec)?;

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

fn resolved_job_spec_parts(spec: &JobSpec) -> Result<ResolvedJobSpecParts, CronError> {
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
            route.as_token().clone(),
            headers.as_map().clone(),
            ttl_sec.map(|ttl_sec| ttl_sec.get()),
            source
                .as_ref()
                .map(|source| source.subject().as_str().to_string()),
        ),
    };

    Ok(ResolvedJobSpecParts {
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::{TimeZone, Utc};

    use super::super::{
        DeliveryHeaders, DeliveryRoute, JobEnabledState, SamplingSource, TtlSeconds,
    };
    use super::*;

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

    fn base_job() -> JobSpec {
        JobSpec {
            id: job_id("heartbeat"),
            state: JobEnabledState::Enabled,
            schedule: ScheduleSpec::Every { every_sec: 30 },
            delivery: DeliverySpec::NatsEvent {
                route: route("agent.run"),
                headers: DeliveryHeaders::new(BTreeMap::from([(
                    "x-kind".to_string(),
                    "heartbeat".to_string(),
                )]))
                .unwrap(),
                ttl_sec: Some(ttl(15)),
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
            route: route("agent.run"),
            headers: DeliveryHeaders::default(),
            ttl_sec: None,
            source: Some(source("sensors.latest")),
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
}
