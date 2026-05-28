use std::str::FromStr;

use async_nats::{HeaderMap, HeaderValue};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use rrule::RRuleSet;
use trogon_nats::{DottedNatsToken, NatsToken};

use crate::{
    DeliveryKind, SamplingSourceKind, ScheduleKind,
    error::{ScheduleSpecError, SchedulerError},
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
pub struct ResolvedSchedule {
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

struct ResolvedScheduleParts {
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

impl ResolvedSchedule {
    pub fn from_event(job_id: &str, job: &v1::ScheduleAdded) -> Result<Self, SchedulerError> {
        Self::from_event_at(job_id, job, Utc::now())
    }

    pub fn from_event_at(job_id: &str, job: &v1::ScheduleAdded, now: DateTime<Utc>) -> Result<Self, SchedulerError> {
        let ResolvedScheduleParts {
            job_id,
            enabled,
            route,
            schedule_expression,
            timezone,
            ttl_sec,
            source_subject,
            headers,
            body,
        } = resolved_job_parts(job_id, job, now)?;

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

fn resolved_job_parts(
    job_id: &str,
    job: &v1::ScheduleAdded,
    now: DateTime<Utc>,
) -> Result<ResolvedScheduleParts, SchedulerError> {
    let job_id = parse_job_id(job_id)?;
    let schedule = job.schedule.as_option().ok_or_else(|| {
        SchedulerError::event_source(
            "scheduler received job details without schedule",
            std::io::Error::other("missing schedule"),
        )
    })?;
    let delivery = job.delivery.as_option().ok_or_else(|| {
        SchedulerError::event_source(
            "scheduler received job details without delivery",
            std::io::Error::other("missing delivery"),
        )
    })?;
    let message = job.message.as_option().ok_or_else(|| {
        SchedulerError::event_source(
            "scheduler received job details without message",
            std::io::Error::other("missing message"),
        )
    })?;
    let (schedule_expression, timezone) = schedule_parts(schedule, now)?;
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

    Ok(ResolvedScheduleParts {
        job_id,
        enabled: job.status != v1::ScheduleStatus::SCHEDULE_STATUS_DISABLED,
        route,
        schedule_expression,
        timezone,
        ttl_sec,
        source_subject,
        headers,
        body,
    })
}

fn parse_job_id(id: &str) -> Result<NatsToken, SchedulerError> {
    NatsToken::new(id).map_err(|source| {
        SchedulerError::invalid_job_spec(ScheduleSpecError::InvalidId {
            id: id.to_string(),
            source,
        })
    })
}

fn schedule_parts(schedule: &v1::Schedule, now: DateTime<Utc>) -> Result<(String, Option<String>), SchedulerError> {
    match schedule.kind.as_ref() {
        Some(ScheduleKind::At(inner)) => Ok((format!("@at {}", inner.at), None)),
        Some(ScheduleKind::Every(inner)) => {
            let every_sec = inner.every_sec;
            if every_sec == 0 {
                return Err(SchedulerError::invalid_job_spec(
                    ScheduleSpecError::EverySecondsMustBePositive,
                ));
            }
            Ok((format!("@every {every_sec}s"), None))
        }
        Some(ScheduleKind::Cron(inner)) => {
            let expr = inner.expr.clone();
            cron::Schedule::from_str(&expr).map_err(|source| {
                SchedulerError::invalid_job_spec(ScheduleSpecError::InvalidCronExpression {
                    expr: expr.clone(),
                    source: Box::new(source),
                })
            })?;
            let timezone = (!inner.timezone.is_empty()).then(|| inner.timezone.clone());
            Ok((expr, validate_timezone(timezone)?))
        }
        Some(ScheduleKind::Rrule(inner)) => {
            let next = next_rrule_occurrence(inner, now)?;
            Ok((format!("@at {}", next.to_rfc3339()), None))
        }
        None => Err(SchedulerError::event_source(
            "scheduler received job details without a schedule kind",
            std::io::Error::other("missing schedule kind"),
        )),
    }
}

fn next_rrule_occurrence(schedule: &v1::RRuleSchedule, now: DateTime<Utc>) -> Result<DateTime<Utc>, SchedulerError> {
    let timezone = (!schedule.timezone.is_empty()).then(|| schedule.timezone.clone());
    let timezone = validate_timezone(timezone)?;
    let timezone_value = parse_rrule_timezone(timezone.as_deref())?;
    let set_text = rrule_set_text(schedule, timezone.as_deref(), timezone_value)?;
    let set = RRuleSet::from_str(&set_text).map_err(|source| {
        SchedulerError::invalid_job_spec(ScheduleSpecError::InvalidRRule {
            rrule: schedule.rrule.clone(),
            source: Box::new(source),
        })
    })?;
    let after = now.with_timezone(&timezone_value);
    let next = set.after(after).all(1).dates.into_iter().next().ok_or_else(|| {
        SchedulerError::invalid_job_spec(ScheduleSpecError::RRuleHasNoNextOccurrence {
            rrule: schedule.rrule.clone(),
        })
    })?;

    Ok(next.with_timezone(&Utc))
}

fn rrule_set_text(
    schedule: &v1::RRuleSchedule,
    timezone_name: Option<&str>,
    timezone: rrule::Tz,
) -> Result<String, SchedulerError> {
    let rrule = normalize_rrule_value(&schedule.rrule)?;
    let mut lines = Vec::with_capacity(4);
    lines.push(dtstart_line(&schedule.dtstart, timezone_name, timezone)?);
    lines.push(format!("RRULE:{rrule}"));
    if let Some(line) = date_list_line("RDATE", &schedule.rdate)? {
        lines.push(line);
    }
    if let Some(line) = date_list_line("EXDATE", &schedule.exdate)? {
        lines.push(line);
    }
    Ok(lines.join("\n"))
}

fn normalize_rrule_value(raw: &str) -> Result<String, SchedulerError> {
    let trimmed = raw.trim();
    let without_prefix = strip_rrule_prefix(trimmed);
    if without_prefix.is_empty()
        || without_prefix.contains('\n')
        || without_prefix.contains('\r')
        || without_prefix.chars().any(char::is_control)
    {
        return Err(SchedulerError::invalid_job_spec(ScheduleSpecError::InvalidRRule {
            rrule: raw.to_string(),
            source: Box::new(std::io::Error::other("empty or multi-line RRULE")),
        }));
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

fn parse_rrule_timezone(timezone: Option<&str>) -> Result<rrule::Tz, SchedulerError> {
    match timezone {
        Some(timezone) => chrono_tz::Tz::from_str(timezone).map(rrule::Tz::from).map_err(|_| {
            SchedulerError::invalid_job_spec(ScheduleSpecError::InvalidTimezone {
                timezone: timezone.to_string(),
            })
        }),
        None => Ok(rrule::Tz::UTC),
    }
}

fn dtstart_line(value: &str, timezone_name: Option<&str>, timezone: rrule::Tz) -> Result<String, SchedulerError> {
    let parsed = parse_rrule_datetime("dtstart", value)?;
    match timezone_name {
        Some(timezone_name) => Ok(format!(
            "DTSTART;TZID={timezone_name}:{}",
            parsed.with_timezone(&timezone).format("%Y%m%dT%H%M%S")
        )),
        None => Ok(format!(
            "DTSTART:{}",
            parsed.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ")
        )),
    }
}

fn date_list_line(name: &'static str, values: &[String]) -> Result<Option<String>, SchedulerError> {
    if values.is_empty() {
        return Ok(None);
    }

    let values = values
        .iter()
        .map(|value| {
            parse_rrule_datetime(name, value)
                .map(|parsed| parsed.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ").to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Some(format!("{name};VALUE=DATE-TIME:{}", values.join(","))))
}

fn parse_rrule_datetime(field: &'static str, value: &str) -> Result<DateTime<chrono::FixedOffset>, SchedulerError> {
    DateTime::parse_from_rfc3339(value).map_err(|source| {
        SchedulerError::invalid_job_spec(ScheduleSpecError::InvalidRRuleDateTime {
            field,
            value: value.to_string(),
            source: Box::new(source),
        })
    })
}

fn delivery_parts(delivery: &v1::Delivery) -> Result<(DottedNatsToken, Option<u64>, Option<String>), SchedulerError> {
    match delivery.kind.as_ref() {
        Some(DeliveryKind::NatsEvent(inner)) => {
            let route = inner.route.clone();
            Ok((
                DottedNatsToken::new(&route).map_err(|source| {
                    SchedulerError::invalid_job_spec(ScheduleSpecError::InvalidRoute {
                        route: route.clone(),
                        source,
                    })
                })?,
                inner.ttl_sec,
                inner.source.as_option().map(source_subject).transpose()?,
            ))
        }
        None => Err(SchedulerError::event_source(
            "scheduler received job details without a delivery kind",
            std::io::Error::other("missing delivery kind"),
        )),
    }
}

fn source_subject(source: &v1::SamplingSource) -> Result<String, SchedulerError> {
    match source.kind.as_ref() {
        Some(SamplingSourceKind::LatestFromSubject(inner)) => Ok(inner.subject.clone()),
        None => Err(SchedulerError::event_source(
            "scheduler received job details without a sampling source kind",
            std::io::Error::other("missing sampling source kind"),
        )),
    }
}

fn validate_timezone(timezone: Option<String>) -> Result<Option<String>, SchedulerError> {
    let Some(timezone) = timezone else {
        return Ok(None);
    };

    let trimmed = timezone.trim();
    if trimmed.is_empty() || trimmed != timezone || timezone.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
        return Err(SchedulerError::invalid_job_spec(ScheduleSpecError::InvalidTimezone {
            timezone,
        }));
    }
    Ok(Some(timezone))
}

fn validate_scheduler_headers(headers: &[(String, String)]) -> Result<(), SchedulerError> {
    for (name, _) in headers {
        if RESERVED_SCHEDULE_HEADERS
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(name))
        {
            return Err(SchedulerError::invalid_job_spec(
                ScheduleSpecError::ReservedHeaderName { name: name.clone() },
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use buffa::MessageField;

    use super::*;
    use crate::commands::domain::proto_timestamp_rfc3339;

    fn base_job() -> v1::ScheduleAdded {
        v1::ScheduleAdded {
            schedule_id: "heartbeat".to_string(),
            added_at: proto_timestamp_rfc3339("2026-05-22T00:00:00+00:00").unwrap(),
            status: v1::ScheduleStatus::SCHEDULE_STATUS_ENABLED,
            schedule: MessageField::some(every_schedule(30)),
            delivery: MessageField::some(nats_delivery(None)),
            message: MessageField::some(message(r#"{"kind":"heartbeat"}"#, [("x-kind", "heartbeat")])),
            added_by: buffa::MessageField::some(trogonai_proto::actor::v1alpha1::ActorId {
                value: "test-actor".to_string(),
            }),
        }
    }

    fn every_schedule(every_sec: u64) -> v1::Schedule {
        v1::Schedule {
            kind: Some(v1::EverySchedule { every_sec }.into()),
        }
    }

    fn at_schedule(at: &str) -> v1::Schedule {
        v1::Schedule {
            kind: Some(v1::AtSchedule { at: at.to_string() }.into()),
        }
    }

    fn cron_schedule(expr: &str, timezone: Option<&str>) -> v1::Schedule {
        v1::Schedule {
            kind: Some(
                v1::CronSchedule {
                    expr: expr.to_string(),
                    timezone: timezone.unwrap_or_default().to_string(),
                }
                .into(),
            ),
        }
    }

    fn rrule_schedule(
        dtstart: &str,
        rrule: &str,
        timezone: Option<&str>,
        rdate: &[&str],
        exdate: &[&str],
    ) -> v1::Schedule {
        v1::Schedule {
            kind: Some(
                v1::RRuleSchedule {
                    dtstart: dtstart.to_string(),
                    rrule: rrule.to_string(),
                    timezone: timezone.unwrap_or_default().to_string(),
                    rdate: rdate.iter().map(|value| value.to_string()).collect(),
                    exdate: exdate.iter().map(|value| value.to_string()).collect(),
                }
                .into(),
            ),
        }
    }

    fn nats_delivery(source: Option<&str>) -> v1::Delivery {
        v1::Delivery {
            kind: Some(
                v1::NatsEventDelivery {
                    route: "agent.run".to_string(),
                    ttl_sec: Some(15),
                    source: source
                        .map(|subject| v1::SamplingSource {
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

    fn message<const N: usize>(content: &str, headers: [(&str, &str); N]) -> v1::Message {
        v1::Message {
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
        let resolved = ResolvedSchedule::from_event("heartbeat", &job).unwrap();
        let headers = resolved.schedule_headers();

        assert_eq!(resolved.schedule_subject(), "scheduler.schedules.heartbeat");
        assert_eq!(resolved.target_subject(), "scheduler.fire.agent.run.heartbeat");
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

        let resolved = ResolvedSchedule::from_event("heartbeat", &job).unwrap();
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

        let resolved = ResolvedSchedule::from_event("heartbeat", &job).unwrap();

        assert_eq!(
            resolved
                .schedule_headers()
                .get(NATS_SCHEDULE)
                .map(|value| value.as_str()),
            Some("@at 2026-04-11T12:00:00+00:00")
        );
    }

    #[test]
    fn rrule_schedule_maps_to_next_one_shot_at_schedule() {
        let mut job = base_job();
        job.schedule = MessageField::some(rrule_schedule(
            "2026-05-24T09:00:00+00:00",
            "FREQ=DAILY;COUNT=3",
            None,
            &[],
            &[],
        ));
        let now = "2026-05-24T08:00:00+00:00".parse::<DateTime<Utc>>().unwrap();

        let resolved = ResolvedSchedule::from_event_at("heartbeat", &job, now).unwrap();

        assert_eq!(
            resolved
                .schedule_headers()
                .get(NATS_SCHEDULE)
                .map(|value| value.as_str()),
            Some("@at 2026-05-24T09:00:00+00:00")
        );
    }

    #[test]
    fn rrule_schedule_expands_with_timezone_wall_clock() {
        let mut job = base_job();
        job.schedule = MessageField::some(rrule_schedule(
            "2026-05-24T13:00:00+00:00",
            "FREQ=DAILY;COUNT=3",
            Some("America/New_York"),
            &[],
            &[],
        ));
        let now = "2026-05-24T13:30:00+00:00".parse::<DateTime<Utc>>().unwrap();

        let resolved = ResolvedSchedule::from_event_at("heartbeat", &job, now).unwrap();

        assert_eq!(
            resolved
                .schedule_headers()
                .get(NATS_SCHEDULE)
                .map(|value| value.as_str()),
            Some("@at 2026-05-25T13:00:00+00:00")
        );
    }

    #[test]
    fn rrule_schedule_applies_exdates() {
        let mut job = base_job();
        job.schedule = MessageField::some(rrule_schedule(
            "2026-05-24T09:00:00+00:00",
            "FREQ=DAILY;COUNT=3",
            None,
            &[],
            &["2026-05-25T09:00:00+00:00"],
        ));
        let now = "2026-05-24T10:00:00+00:00".parse::<DateTime<Utc>>().unwrap();

        let resolved = ResolvedSchedule::from_event_at("heartbeat", &job, now).unwrap();

        assert_eq!(
            resolved
                .schedule_headers()
                .get(NATS_SCHEDULE)
                .map(|value| value.as_str()),
            Some("@at 2026-05-26T09:00:00+00:00")
        );
    }

    #[test]
    fn expired_rrule_schedule_is_rejected() {
        let mut job = base_job();
        job.schedule = MessageField::some(rrule_schedule(
            "2026-05-24T09:00:00+00:00",
            "FREQ=DAILY;COUNT=1",
            None,
            &[],
            &[],
        ));
        let now = "2026-05-24T10:00:00+00:00".parse::<DateTime<Utc>>().unwrap();

        let error = ResolvedSchedule::from_event_at("heartbeat", &job, now).unwrap_err();
        assert!(error.to_string().contains("no next occurrence"));
    }

    #[test]
    fn source_uses_placeholder_body() {
        let mut job = base_job();
        job.delivery = MessageField::some(nats_delivery(Some("sensors.latest")));

        let resolved = ResolvedSchedule::from_event("heartbeat", &job).unwrap();

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
        ResolvedSchedule::from_event("heartbeat", &job).unwrap();
    }

    #[test]
    fn resolved_job_accessors_expose_validated_values() {
        let mut job = base_job();
        job.status = v1::ScheduleStatus::SCHEDULE_STATUS_DISABLED;

        let resolved = ResolvedSchedule::from_event("heartbeat", &job).unwrap();

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

        let error = ResolvedSchedule::from_event("heartbeat", &job).unwrap_err();
        assert!(error.to_string().contains("every_sec"));
    }

    #[test]
    fn invalid_cron_expression_is_rejected() {
        let mut job = base_job();
        job.schedule = MessageField::some(cron_schedule("not-a-cron", None));

        let error = ResolvedSchedule::from_event("heartbeat", &job).unwrap_err();
        assert!(error.to_string().contains("cron expression"));
    }

    #[test]
    fn invalid_timezone_is_rejected() {
        let mut job = base_job();
        job.schedule = MessageField::some(cron_schedule("0 * * * * *", Some(" America/New_York ")));

        let error = ResolvedSchedule::from_event("heartbeat", &job).unwrap_err();
        assert!(error.to_string().contains("timezone"));
    }

    #[test]
    fn reserved_scheduler_header_is_rejected_during_schedule_resolution() {
        let mut job = base_job();
        job.message = MessageField::some(message("{}", [("Nats-Schedule-Target", "scheduler.fire.evil.target")]));

        let error = ResolvedSchedule::from_event("heartbeat", &job).unwrap_err();
        assert!(error.to_string().contains("reserved"));
    }
}
