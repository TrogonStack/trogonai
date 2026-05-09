use std::str::FromStr;

use async_nats::{HeaderMap, HeaderValue};
use bytes::Bytes;
use trogon_nats::{DottedNatsToken, NatsToken};

use crate::{
    error::{CronError, JobSpecError},
    kv::{FIRE_SUBJECT_PREFIX, SCHEDULE_SUBJECT_PREFIX},
    v1,
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
pub struct ResolvedJob {
    job_id: NatsToken,
    enabled: bool,
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

struct ResolvedJobParts {
    job_id: NatsToken,
    enabled: bool,
    route: DottedNatsToken,
    schedule_expression: String,
    timezone: Option<String>,
    ttl_sec: Option<u64>,
    source_subject: Option<String>,
    headers: Vec<(String, String)>,
    body: Bytes,
}

impl ResolvedJob {
    pub fn from_event(job_id: &str, job: &v1::JobDetails) -> Result<Self, CronError> {
        let ResolvedJobParts {
            job_id,
            enabled,
            route,
            schedule_expression,
            timezone,
            ttl_sec,
            source_subject,
            headers,
            body,
        } = resolved_job_parts(job_id, job)?;

        let schedule_subject = format!("{SCHEDULE_SUBJECT_PREFIX}{}", job_id.as_str());
        let target_subject = format!("{FIRE_SUBJECT_PREFIX}{}.{}", route.as_str(), job_id.as_str());

        Ok(Self {
            job_id,
            enabled,
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

    pub fn id(&self) -> &str {
        self.job_id.as_str()
    }

    pub fn enabled(&self) -> bool {
        self.enabled
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

fn resolved_job_parts(job_id: &str, job: &v1::JobDetails) -> Result<ResolvedJobParts, CronError> {
    let job_id = parse_job_id(job_id)?;
    let schedule = job.schedule.as_option().ok_or_else(|| {
        CronError::event_source(
            "scheduler received job details without schedule",
            std::io::Error::other("missing schedule"),
        )
    })?;
    let delivery = job.delivery.as_option().ok_or_else(|| {
        CronError::event_source(
            "scheduler received job details without delivery",
            std::io::Error::other("missing delivery"),
        )
    })?;
    let message = job.message.as_option().ok_or_else(|| {
        CronError::event_source(
            "scheduler received job details without message",
            std::io::Error::other("missing message"),
        )
    })?;
    let (schedule_expression, timezone) = schedule_parts(schedule)?;
    let (route, ttl_sec, source_subject) = delivery_parts(delivery)?;
    let headers = message
        .headers
        .iter()
        .map(|header| (header.name.clone(), header.value.clone()))
        .collect::<Vec<_>>();
    validate_scheduler_headers(&headers)?;
    let body = if source_subject.is_some() {
        Bytes::from_static(br#"{}"#)
    } else {
        Bytes::from(message.content.clone())
    };

    Ok(ResolvedJobParts {
        job_id,
        enabled: job.status != v1::JobStatus::JOB_STATUS_DISABLED,
        route,
        schedule_expression,
        timezone,
        ttl_sec,
        source_subject,
        headers,
        body,
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

fn schedule_parts(schedule: &v1::JobSchedule) -> Result<(String, Option<String>), CronError> {
    match schedule.kind.as_ref() {
        Some(v1::__buffa::oneof::job_schedule::Kind::At(inner)) => Ok((format!("@at {}", inner.at), None)),
        Some(v1::__buffa::oneof::job_schedule::Kind::Every(inner)) => {
            let every_sec = inner.every_sec;
            if every_sec == 0 {
                return Err(CronError::invalid_job_spec(JobSpecError::EverySecondsMustBePositive));
            }
            Ok((format!("@every {every_sec}s"), None))
        }
        Some(v1::__buffa::oneof::job_schedule::Kind::Cron(inner)) => {
            let expr = inner.expr.clone();
            cron::Schedule::from_str(&expr).map_err(|source| {
                CronError::invalid_job_spec(JobSpecError::InvalidCronExpression {
                    expr: expr.clone(),
                    source: Box::new(source),
                })
            })?;
            let timezone = (!inner.timezone.is_empty()).then(|| inner.timezone.clone());
            Ok((expr, validate_timezone(timezone)?))
        }
        None => Err(CronError::event_source(
            "scheduler received job details without a schedule kind",
            std::io::Error::other("missing schedule kind"),
        )),
    }
}

fn delivery_parts(delivery: &v1::JobDelivery) -> Result<(DottedNatsToken, Option<u64>, Option<String>), CronError> {
    match delivery.kind.as_ref() {
        Some(v1::__buffa::oneof::job_delivery::Kind::NatsEvent(inner)) => {
            let route = inner.route.clone();
            Ok((
                DottedNatsToken::new(&route).map_err(|source| {
                    CronError::invalid_job_spec(JobSpecError::InvalidRoute {
                        route: route.clone(),
                        source,
                    })
                })?,
                inner.ttl_sec,
                inner.source.as_option().map(source_subject).transpose()?,
            ))
        }
        None => Err(CronError::event_source(
            "scheduler received job details without a delivery kind",
            std::io::Error::other("missing delivery kind"),
        )),
    }
}

fn source_subject(source: &v1::JobSamplingSource) -> Result<String, CronError> {
    match source.kind.as_ref() {
        Some(v1::__buffa::oneof::job_sampling_source::Kind::LatestFromSubject(inner)) => Ok(inner.subject.clone()),
        None => Err(CronError::event_source(
            "scheduler received job details without a sampling source kind",
            std::io::Error::other("missing sampling source kind"),
        )),
    }
}

fn validate_timezone(timezone: Option<String>) -> Result<Option<String>, CronError> {
    let Some(timezone) = timezone else {
        return Ok(None);
    };

    let trimmed = timezone.trim();
    if trimmed.is_empty() || trimmed != timezone || timezone.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
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
    use buffa::MessageField;

    use super::*;

    fn base_job() -> v1::JobDetails {
        v1::JobDetails {
            status: v1::JobStatus::JOB_STATUS_ENABLED,
            schedule: MessageField::some(every_schedule(30)),
            delivery: MessageField::some(nats_delivery(None)),
            message: MessageField::some(message(r#"{"kind":"heartbeat"}"#, [("x-kind", "heartbeat")])),
        }
    }

    fn every_schedule(every_sec: u64) -> v1::JobSchedule {
        v1::JobSchedule {
            kind: Some(v1::EverySchedule { every_sec }.into()),
        }
    }

    fn at_schedule(at: &str) -> v1::JobSchedule {
        v1::JobSchedule {
            kind: Some(v1::AtSchedule { at: at.to_string() }.into()),
        }
    }

    fn cron_schedule(expr: &str, timezone: Option<&str>) -> v1::JobSchedule {
        v1::JobSchedule {
            kind: Some(
                v1::CronSchedule {
                    expr: expr.to_string(),
                    timezone: timezone.unwrap_or_default().to_string(),
                }
                .into(),
            ),
        }
    }

    fn nats_delivery(source: Option<&str>) -> v1::JobDelivery {
        v1::JobDelivery {
            kind: Some(
                v1::NatsEventDelivery {
                    route: "agent.run".to_string(),
                    ttl_sec: Some(15),
                    source: source
                        .map(|subject| v1::JobSamplingSource {
                            kind: Some(
                                v1::LatestFromSubjectSampling {
                                    subject: subject.to_string(),
                                }
                                .into(),
                            ),
                        })
                        .map(MessageField::some)
                        .unwrap_or_else(MessageField::none),
                }
                .into(),
            ),
        }
    }

    fn message<const N: usize>(content: &str, headers: [(&str, &str); N]) -> v1::JobMessage {
        v1::JobMessage {
            content: content.to_string(),
            headers: headers
                .into_iter()
                .map(|(name, value)| v1::Header {
                    name: name.to_string(),
                    value: value.to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn resolved_job_derives_subjects_and_headers() {
        let job = base_job();
        let resolved = ResolvedJob::from_event("heartbeat", &job).unwrap();
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
        job.message = MessageField::some(message("{}", [("x-kind", "heartbeat"), ("x-kind", "retry")]));

        let resolved = ResolvedJob::from_event("heartbeat", &job).unwrap();
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
        job.schedule = MessageField::some(at_schedule("2026-04-11T12:00:00+00:00"));

        let resolved = ResolvedJob::from_event("heartbeat", &job).unwrap();

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
        job.delivery = MessageField::some(nats_delivery(Some("sensors.latest")));

        let resolved = ResolvedJob::from_event("heartbeat", &job).unwrap();

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
        let job = base_job();
        ResolvedJob::from_event("heartbeat", &job).unwrap();
    }

    #[test]
    fn resolved_job_accessors_expose_validated_values() {
        let mut job = base_job();
        job.status = v1::JobStatus::JOB_STATUS_DISABLED;

        let resolved = ResolvedJob::from_event("heartbeat", &job).unwrap();

        assert_eq!(resolved.id(), "heartbeat");
        assert!(!resolved.enabled());
        assert_eq!(resolved.route(), "agent.run");
        assert_eq!(resolved.schedule_body(), Bytes::from_static(br#"{"kind":"heartbeat"}"#));
        assert_eq!(resolved.source_subject(), None);
    }

    #[test]
    fn zero_every_seconds_is_rejected() {
        let mut job = base_job();
        job.schedule = MessageField::some(every_schedule(0));

        let error = ResolvedJob::from_event("heartbeat", &job).unwrap_err();
        assert!(error.to_string().contains("every_sec"));
    }

    #[test]
    fn invalid_cron_expression_is_rejected() {
        let mut job = base_job();
        job.schedule = MessageField::some(cron_schedule("not-a-cron", None));

        let error = ResolvedJob::from_event("heartbeat", &job).unwrap_err();
        assert!(error.to_string().contains("cron expression"));
    }

    #[test]
    fn invalid_timezone_is_rejected() {
        let mut job = base_job();
        job.schedule = MessageField::some(cron_schedule("0 * * * * *", Some(" America/New_York ")));

        let error = ResolvedJob::from_event("heartbeat", &job).unwrap_err();
        assert!(error.to_string().contains("timezone"));
    }

    #[test]
    fn reserved_scheduler_header_is_rejected_during_schedule_resolution() {
        let mut job = base_job();
        job.message = MessageField::some(message("{}", [("Nats-Schedule-Target", "cron.fire.evil.target")]));

        let error = ResolvedJob::from_event("heartbeat", &job).unwrap_err();
        assert!(error.to_string().contains("reserved"));
    }
}
