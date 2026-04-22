use std::str::FromStr;

use async_nats::{HeaderMap, HeaderValue};
use bytes::Bytes;
use trogon_nats::{DottedNatsToken, NatsToken};

use crate::{
    CronJob,
    error::{CronError, JobSpecError},
    events::{JobEventDelivery, JobEventSchedule, JobEventStatus},
    kv::{FIRE_SUBJECT_PREFIX, SCHEDULE_SUBJECT_PREFIX},
};

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
    job: CronJob,
    job_id: NatsToken,
    route: DottedNatsToken,
    schedule_subject: String,
    target_subject: String,
    schedule_expression: String,
    timezone: Option<String>,
    ttl_sec: Option<u64>,
    source_subject: Option<String>,
    headers: Vec<(String, String)>,
    body: Bytes,
}

struct ResolvedJobSpecParts {
    job_id: NatsToken,
    route: DottedNatsToken,
    schedule_expression: String,
    timezone: Option<String>,
    ttl_sec: Option<u64>,
    source_subject: Option<String>,
    headers: Vec<(String, String)>,
}

impl TryFrom<&CronJob> for ResolvedJobSpec {
    type Error = CronError;

    fn try_from(job: &CronJob) -> Result<Self, Self::Error> {
        let ResolvedJobSpecParts {
            job_id,
            route,
            schedule_expression,
            timezone,
            ttl_sec,
            source_subject,
            headers,
        } = resolved_job_spec_parts(job)?;

        let schedule_subject = format!("{SCHEDULE_SUBJECT_PREFIX}{}", job_id.as_str());
        let target_subject = format!("{FIRE_SUBJECT_PREFIX}{}.{}", route.as_str(), job_id.as_str());
        let body = if source_subject.is_some() {
            Bytes::from_static(br#"{}"#)
        } else {
            Bytes::copy_from_slice(job.message.content.as_slice())
        };

        Ok(Self {
            job: job.clone(),
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

fn resolved_job_spec_parts(job: &CronJob) -> Result<ResolvedJobSpecParts, CronError> {
    let job_id = parse_job_id(&job.id)?;
    let schedule_expression = schedule_expression(&job.schedule)?;
    let timezone = match &job.schedule {
        JobEventSchedule::Cron { timezone, .. } => validate_timezone(timezone.clone())?,
        JobEventSchedule::At { .. } | JobEventSchedule::Every { .. } => None,
    };
    let (route, ttl_sec, source_subject) = match &job.delivery {
        JobEventDelivery::NatsEvent { route, ttl_sec, source } => (
            DottedNatsToken::new(route).map_err(|source| {
                CronError::invalid_job_spec(JobSpecError::InvalidRoute {
                    route: route.clone(),
                    source,
                })
            })?,
            *ttl_sec,
            source.as_ref().map(|source| match source {
                crate::JobEventSamplingSource::LatestFromSubject { subject } => subject.clone(),
            }),
        ),
    };
    let headers = job.message.headers.as_slice().to_vec();
    validate_scheduler_headers(&headers)?;

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

fn parse_job_id(id: &str) -> Result<NatsToken, CronError> {
    NatsToken::new(id).map_err(|source| {
        CronError::invalid_job_spec(JobSpecError::InvalidId {
            id: id.to_string(),
            source,
        })
    })
}

impl ResolvedJobSpec {
    pub fn job(&self) -> &CronJob {
        &self.job
    }

    pub fn id(&self) -> &str {
        self.job_id.as_str()
    }

    pub fn enabled(&self) -> bool {
        matches!(self.job.status, JobEventStatus::Enabled)
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
        headers.insert(NATS_SCHEDULE, HeaderValue::from(self.schedule_expression.as_str()));
        headers.insert(NATS_SCHEDULE_TARGET, HeaderValue::from(self.target_subject.as_str()));

        if let Some(timezone) = &self.timezone {
            headers.insert(NATS_SCHEDULE_TIME_ZONE, HeaderValue::from(timezone.as_str()));
        }
        if let Some(ttl_sec) = self.ttl_sec {
            headers.insert(NATS_SCHEDULE_TTL, HeaderValue::from(format!("{ttl_sec}s")));
        }
        if let Some(source_subject) = &self.source_subject {
            headers.insert(NATS_SCHEDULE_SOURCE, HeaderValue::from(source_subject.as_str()));
        }
        for (name, value) in &self.headers {
            headers.append(name.as_str(), value.as_str());
        }

        headers
    }
}

fn schedule_expression(schedule: &JobEventSchedule) -> Result<String, CronError> {
    match schedule {
        JobEventSchedule::At { at } => Ok(format!("@at {}", at.to_rfc3339())),
        JobEventSchedule::Every { every_sec } => {
            if *every_sec == 0 {
                return Err(CronError::invalid_job_spec(JobSpecError::EverySecondsMustBePositive));
            }
            Ok(format!("@every {every_sec}s"))
        }
        JobEventSchedule::Cron { expr, .. } => {
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
    if trimmed.is_empty() || trimmed != timezone || trimmed.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
        return Err(CronError::invalid_job_spec(JobSpecError::InvalidTimezone { timezone }));
    }
    Ok(Some(timezone))
}

fn validate_scheduler_headers(headers: &[(String, String)]) -> Result<(), CronError> {
    for (name, _) in headers {
        if RESERVED_SCHEDULE_HEADERS
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(name))
        {
            return Err(CronError::invalid_job_spec(JobSpecError::ReservedHeaderName {
                name: name.clone(),
            }));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::events::{JobEventDelivery, JobEventSamplingSource, JobEventSchedule, JobEventStatus};
    use crate::{MessageContent, MessageEnvelope, MessageHeaders};

    fn base_job() -> CronJob {
        CronJob {
            id: "heartbeat".to_string(),
            status: JobEventStatus::Enabled,
            schedule: JobEventSchedule::Every { every_sec: 30 },
            delivery: JobEventDelivery::NatsEvent {
                route: "agent.run".to_string(),
                ttl_sec: Some(15),
                source: None,
            },
            message: MessageEnvelope {
                content: MessageContent::from_static(br#"{"kind":"heartbeat"}"#),
                headers: MessageHeaders::new([("x-kind", "heartbeat")]).unwrap(),
            },
        }
    }

    #[test]
    fn resolved_job_derives_subjects_and_headers() {
        let resolved = ResolvedJobSpec::try_from(&base_job()).unwrap();
        let headers = resolved.schedule_headers();

        assert_eq!(resolved.schedule_subject(), "cron.schedules.heartbeat");
        assert_eq!(resolved.target_subject(), "cron.fire.agent.run.heartbeat");
        assert_eq!(
            headers.get(NATS_SCHEDULE).map(|value| value.as_str()),
            Some("@every 30s")
        );
        assert_eq!(headers.get(NATS_SCHEDULE_TTL).map(|value| value.as_str()), Some("15s"));
    }

    #[test]
    fn resolved_job_preserves_duplicate_headers() {
        let mut job = base_job();
        job.message.headers = MessageHeaders::new([("x-kind", "heartbeat"), ("x-kind", "retry")]).unwrap();

        let resolved = ResolvedJobSpec::try_from(&job).unwrap();
        let schedule_headers = resolved.schedule_headers();
        let values = schedule_headers
            .get_all("x-kind")
            .map(|value| value.as_str())
            .collect::<Vec<_>>();

        assert_eq!(values, vec!["heartbeat", "retry"]);
    }

    #[test]
    fn at_schedule_maps_to_rfc3339() {
        let mut job = base_job();
        job.schedule = JobEventSchedule::At {
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
        job.delivery = JobEventDelivery::NatsEvent {
            route: "agent.run".to_string(),
            ttl_sec: None,
            source: Some(JobEventSamplingSource::LatestFromSubject {
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
    fn resolved_job_accepts_valid_job() {
        ResolvedJobSpec::try_from(&base_job()).unwrap();
    }

    #[test]
    fn resolved_job_accessors_expose_validated_values() {
        let mut job = base_job();
        job.status = JobEventStatus::Disabled;

        let resolved = ResolvedJobSpec::try_from(&job).unwrap();

        assert_eq!(resolved.job().id, "heartbeat");
        assert_eq!(resolved.id(), "heartbeat");
        assert!(!resolved.enabled());
        assert_eq!(resolved.route(), "agent.run");
        assert_eq!(resolved.schedule_body(), Bytes::from_static(br#"{"kind":"heartbeat"}"#));
        assert_eq!(resolved.source_subject(), None);
    }

    #[test]
    fn zero_every_seconds_is_rejected() {
        let mut job = base_job();
        job.schedule = JobEventSchedule::Every { every_sec: 0 };

        let error = ResolvedJobSpec::try_from(&job).unwrap_err();
        assert!(error.to_string().contains("every_sec"));
    }

    #[test]
    fn invalid_cron_expression_is_rejected() {
        let mut job = base_job();
        job.schedule = JobEventSchedule::Cron {
            expr: "not-a-cron".to_string(),
            timezone: None,
        };

        let error = ResolvedJobSpec::try_from(&job).unwrap_err();
        assert!(error.to_string().contains("cron expression"));
    }

    #[test]
    fn invalid_timezone_is_rejected() {
        let mut job = base_job();
        job.schedule = JobEventSchedule::Cron {
            expr: "0 * * * * *".to_string(),
            timezone: Some(" America/New_York ".to_string()),
        };

        let error = ResolvedJobSpec::try_from(&job).unwrap_err();
        assert!(error.to_string().contains("timezone"));
    }

    #[test]
    fn reserved_scheduler_header_is_rejected_during_schedule_resolution() {
        let mut job = base_job();
        job.message.headers = MessageHeaders::new([("Nats-Schedule-Target", "cron.fire.evil.target")]).unwrap();

        let error = ResolvedJobSpec::try_from(&job).unwrap_err();
        assert!(error.to_string().contains("reserved"));
    }
}
