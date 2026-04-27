use std::str::FromStr;

use async_nats::{HeaderMap, HeaderValue};
use bytes::Bytes;
use trogon_nats::{DottedNatsToken, NatsToken};

use crate::{
    error::{CronError, JobSpecError},
    kv::{FIRE_SUBJECT_PREFIX, SCHEDULE_SUBJECT_PREFIX},
    proto::v1,
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
    pub fn from_event(job_id: &str, job: v1::JobDetailsView<'_>) -> Result<Self, CronError> {
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

fn resolved_job_parts(job_id: &str, job: v1::JobDetailsView<'_>) -> Result<ResolvedJobParts, CronError> {
    let job_id = parse_job_id(job_id)?;
    let (schedule_expression, timezone) = schedule_parts(job.schedule())?;
    let (route, ttl_sec, source_subject) = delivery_parts(job.delivery())?;
    let headers = job
        .message()
        .headers()
        .iter()
        .map(|header| (header.name().to_string(), header.value().to_string()))
        .collect::<Vec<_>>();
    validate_scheduler_headers(&headers)?;
    let body = if source_subject.is_some() {
        Bytes::from_static(br#"{}"#)
    } else {
        Bytes::from(job.message().content().to_string())
    };

    Ok(ResolvedJobParts {
        job_id,
        enabled: job.status() != v1::JobStatus::Disabled,
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

fn schedule_parts(schedule: v1::JobScheduleView<'_>) -> Result<(String, Option<String>), CronError> {
    match schedule.kind() {
        v1::job_schedule::KindOneof::At(inner) => Ok((format!("@at {}", inner.at()), None)),
        v1::job_schedule::KindOneof::Every(inner) => {
            let every_sec = inner.every_sec();
            if every_sec == 0 {
                return Err(CronError::invalid_job_spec(JobSpecError::EverySecondsMustBePositive));
            }
            Ok((format!("@every {every_sec}s"), None))
        }
        v1::job_schedule::KindOneof::Cron(inner) => {
            let expr = inner.expr().to_string();
            cron::Schedule::from_str(&expr).map_err(|source| {
                CronError::invalid_job_spec(JobSpecError::InvalidCronExpression {
                    expr: expr.clone(),
                    source: Box::new(source),
                })
            })?;
            Ok((
                expr,
                validate_timezone(inner.has_timezone().then(|| inner.timezone().to_string()))?,
            ))
        }
        v1::job_schedule::KindOneof::not_set(_) | _ => Err(CronError::event_source(
            "scheduler received job details without a schedule kind",
            std::io::Error::other("missing schedule kind"),
        )),
    }
}

fn delivery_parts(
    delivery: v1::JobDeliveryView<'_>,
) -> Result<(DottedNatsToken, Option<u64>, Option<String>), CronError> {
    match delivery.kind() {
        v1::job_delivery::KindOneof::NatsEvent(inner) => {
            let route = inner.route().to_string();
            Ok((
                DottedNatsToken::new(&route).map_err(|source| {
                    CronError::invalid_job_spec(JobSpecError::InvalidRoute {
                        route: route.clone(),
                        source,
                    })
                })?,
                inner.has_ttl_sec().then(|| inner.ttl_sec()),
                inner.has_source().then(|| source_subject(inner.source())).transpose()?,
            ))
        }
        v1::job_delivery::KindOneof::not_set(_) | _ => Err(CronError::event_source(
            "scheduler received job details without a delivery kind",
            std::io::Error::other("missing delivery kind"),
        )),
    }
}

fn source_subject(source: v1::JobSamplingSourceView<'_>) -> Result<String, CronError> {
    match source.kind() {
        v1::job_sampling_source::KindOneof::LatestFromSubject(inner) => Ok(inner.subject().to_string()),
        v1::job_sampling_source::KindOneof::not_set(_) | _ => Err(CronError::event_source(
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
    use super::*;

    fn base_job() -> v1::JobDetails {
        let mut job = v1::JobDetails::new();
        job.set_status(v1::JobStatus::Enabled);
        job.set_schedule(every_schedule(30));
        job.set_delivery(nats_delivery(None));
        job.set_message(message(r#"{"kind":"heartbeat"}"#, [("x-kind", "heartbeat")]));
        job
    }

    fn every_schedule(every_sec: u64) -> v1::JobSchedule {
        let mut schedule = v1::JobSchedule::new();
        let mut every = v1::EverySchedule::new();
        every.set_every_sec(every_sec);
        schedule.set_every(every);
        schedule
    }

    fn at_schedule(at: &str) -> v1::JobSchedule {
        let mut schedule = v1::JobSchedule::new();
        let mut inner = v1::AtSchedule::new();
        inner.set_at(at);
        schedule.set_at(inner);
        schedule
    }

    fn cron_schedule(expr: &str, timezone: Option<&str>) -> v1::JobSchedule {
        let mut schedule = v1::JobSchedule::new();
        let mut inner = v1::CronSchedule::new();
        inner.set_expr(expr);
        if let Some(timezone) = timezone {
            inner.set_timezone(timezone);
        }
        schedule.set_cron(inner);
        schedule
    }

    fn nats_delivery(source: Option<&str>) -> v1::JobDelivery {
        let mut delivery = v1::JobDelivery::new();
        let mut nats = v1::NatsEventDelivery::new();
        nats.set_route("agent.run");
        nats.set_ttl_sec(15);
        if let Some(subject) = source {
            let mut sampling = v1::JobSamplingSource::new();
            let mut latest = v1::LatestFromSubjectSampling::new();
            latest.set_subject(subject);
            sampling.set_latest_from_subject(latest);
            nats.set_source(sampling);
        }
        delivery.set_nats_event(nats);
        delivery
    }

    fn message<const N: usize>(content: &str, headers: [(&str, &str); N]) -> v1::JobMessage {
        let mut message = v1::JobMessage::new();
        message.set_content(content);
        for (name, value) in headers {
            let mut header = v1::Header::new();
            header.set_name(name);
            header.set_value(value);
            message.headers_mut().push(header);
        }
        message
    }

    #[test]
    fn resolved_job_derives_subjects_and_headers() {
        let job = base_job();
        let resolved = ResolvedJob::from_event("heartbeat", job.as_view()).unwrap();
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
        job.set_message(message("{}", [("x-kind", "heartbeat"), ("x-kind", "retry")]));

        let resolved = ResolvedJob::from_event("heartbeat", job.as_view()).unwrap();
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
        job.set_schedule(at_schedule("2026-04-11T12:00:00+00:00"));

        let resolved = ResolvedJob::from_event("heartbeat", job.as_view()).unwrap();

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
        job.set_delivery(nats_delivery(Some("sensors.latest")));

        let resolved = ResolvedJob::from_event("heartbeat", job.as_view()).unwrap();

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
        ResolvedJob::from_event("heartbeat", job.as_view()).unwrap();
    }

    #[test]
    fn resolved_job_accessors_expose_validated_values() {
        let mut job = base_job();
        job.set_status(v1::JobStatus::Disabled);

        let resolved = ResolvedJob::from_event("heartbeat", job.as_view()).unwrap();

        assert_eq!(resolved.id(), "heartbeat");
        assert!(!resolved.enabled());
        assert_eq!(resolved.route(), "agent.run");
        assert_eq!(resolved.schedule_body(), Bytes::from_static(br#"{"kind":"heartbeat"}"#));
        assert_eq!(resolved.source_subject(), None);
    }

    #[test]
    fn zero_every_seconds_is_rejected() {
        let mut job = base_job();
        job.set_schedule(every_schedule(0));

        let error = ResolvedJob::from_event("heartbeat", job.as_view()).unwrap_err();
        assert!(error.to_string().contains("every_sec"));
    }

    #[test]
    fn invalid_cron_expression_is_rejected() {
        let mut job = base_job();
        job.set_schedule(cron_schedule("not-a-cron", None));

        let error = ResolvedJob::from_event("heartbeat", job.as_view()).unwrap_err();
        assert!(error.to_string().contains("cron expression"));
    }

    #[test]
    fn invalid_timezone_is_rejected() {
        let mut job = base_job();
        job.set_schedule(cron_schedule("0 * * * * *", Some(" America/New_York ")));

        let error = ResolvedJob::from_event("heartbeat", job.as_view()).unwrap_err();
        assert!(error.to_string().contains("timezone"));
    }

    #[test]
    fn reserved_scheduler_header_is_rejected_during_schedule_resolution() {
        let mut job = base_job();
        job.set_message(message("{}", [("Nats-Schedule-Target", "cron.fire.evil.target")]));

        let error = ResolvedJob::from_event("heartbeat", job.as_view()).unwrap_err();
        assert!(error.to_string().contains("reserved"));
    }
}
