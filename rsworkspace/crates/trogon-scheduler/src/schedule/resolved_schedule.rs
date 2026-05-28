use std::str::FromStr;

use async_nats::{HeaderMap, HeaderValue};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use rrule::RRuleSet;
use trogon_nats::{DottedNatsToken, NatsToken};

use crate::{
    DeliveryKind, ScheduleKind, ScheduleStatusKind, SourceKind,
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
    pub fn from_event(job_id: &str, job: &v1::ScheduleCreated) -> Result<Self, SchedulerError> {
        Self::from_event_at(job_id, job, Utc::now())
    }

    pub fn from_event_at(job_id: &str, job: &v1::ScheduleCreated, now: DateTime<Utc>) -> Result<Self, SchedulerError> {
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
    job: &v1::ScheduleCreated,
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
        let content_bytes = message.content.as_option().map(|c| c.data.clone()).unwrap_or_default();
        Bytes::from(content_bytes)
    };
    let is_paused = matches!(
        job.status.as_option().and_then(|s| s.kind.as_ref()),
        Some(ScheduleStatusKind::Paused(_))
    );

    Ok(ResolvedScheduleParts {
        job_id,
        enabled: !is_paused,
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

fn timestamp_to_rfc3339(ts: &buffa_types::google::protobuf::Timestamp) -> String {
    use chrono::TimeZone;
    chrono::Utc
        .timestamp_opt(ts.seconds, ts.nanos as u32)
        .single()
        .unwrap_or_default()
        .to_rfc3339()
}

fn schedule_parts(schedule: &v1::Schedule, now: DateTime<Utc>) -> Result<(String, Option<String>), SchedulerError> {
    match schedule.kind.as_ref() {
        Some(ScheduleKind::At(inner)) => {
            let at_str = inner.at.as_option().map(timestamp_to_rfc3339).unwrap_or_default();
            Ok((format!("@at {at_str}"), None))
        }
        Some(ScheduleKind::Every(inner)) => {
            let every_sec = inner.every.as_option().map(|d| d.seconds as u64).unwrap_or(0);
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
            let timezone = inner
                .timezone
                .as_option()
                .map(|tz| tz.id.clone())
                .filter(|s| !s.is_empty());
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

fn next_rrule_occurrence(schedule: &v1::schedule::RRule, now: DateTime<Utc>) -> Result<DateTime<Utc>, SchedulerError> {
    let timezone = schedule
        .timezone
        .as_option()
        .map(|tz| tz.id.clone())
        .filter(|s| !s.is_empty());
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
    schedule: &v1::schedule::RRule,
    timezone_name: Option<&str>,
    timezone: rrule::Tz,
) -> Result<String, SchedulerError> {
    let rrule = normalize_rrule_value(&schedule.rrule)?;
    let mut lines = Vec::with_capacity(4);
    lines.push(dtstart_line(schedule.dtstart.as_option(), timezone_name, timezone)?);
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

fn dtstart_line(
    ts: Option<&buffa_types::google::protobuf::Timestamp>,
    timezone_name: Option<&str>,
    timezone: rrule::Tz,
) -> Result<String, SchedulerError> {
    use chrono::TimeZone;
    let dt = ts
        .and_then(|t| Utc.timestamp_opt(t.seconds, t.nanos as u32).single())
        .unwrap_or_default();
    match timezone_name {
        Some(timezone_name) => Ok(format!(
            "DTSTART;TZID={timezone_name}:{}",
            dt.with_timezone(&timezone).format("%Y%m%dT%H%M%S")
        )),
        None => Ok(format!("DTSTART:{}", dt.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ"))),
    }
}

fn date_list_line(
    name: &'static str,
    values: &[buffa_types::google::protobuf::Timestamp],
) -> Result<Option<String>, SchedulerError> {
    if values.is_empty() {
        return Ok(None);
    }
    use chrono::TimeZone;
    let formatted = values
        .iter()
        .map(|ts| {
            Utc.timestamp_opt(ts.seconds, ts.nanos as u32)
                .single()
                .unwrap_or_default()
                .format("%Y%m%dT%H%M%SZ")
                .to_string()
        })
        .collect::<Vec<_>>();

    Ok(Some(format!("{name};VALUE=DATE-TIME:{}", formatted.join(","))))
}

fn delivery_parts(delivery: &v1::Delivery) -> Result<(DottedNatsToken, Option<u64>, Option<String>), SchedulerError> {
    match delivery.kind.as_ref() {
        Some(DeliveryKind::NatsMessage(inner)) => {
            let subject = inner.subject.clone();
            Ok((
                DottedNatsToken::new(&subject).map_err(|source| {
                    SchedulerError::invalid_job_spec(ScheduleSpecError::InvalidRoute {
                        route: subject.clone(),
                        source,
                    })
                })?,
                inner.ttl.as_option().map(|d| d.seconds as u64),
                inner.source.as_option().map(source_subject).transpose()?,
            ))
        }
        None => Err(SchedulerError::event_source(
            "scheduler received job details without a delivery kind",
            std::io::Error::other("missing delivery kind"),
        )),
    }
}

fn source_subject(source: &v1::delivery::nats_message::Source) -> Result<String, SchedulerError> {
    match source.kind.as_ref() {
        Some(SourceKind::LatestFromSubject(inner)) => Ok(inner.subject.clone()),
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
    use buffa_types::google::protobuf::{Duration, Timestamp};

    use super::*;

    fn timestamp_from_str(rfc3339: &str) -> Timestamp {
        let dt = chrono::DateTime::parse_from_rfc3339(rfc3339).unwrap();
        Timestamp {
            seconds: dt.timestamp(),
            nanos: dt.timestamp_subsec_nanos() as i32,
            ..Default::default()
        }
    }

    fn base_job() -> v1::ScheduleCreated {
        v1::ScheduleCreated {
            schedule_id: "heartbeat".to_string(),
            status: MessageField::some(v1::ScheduleStatus {
                kind: Some(v1::schedule_status::Scheduled {}.into()),
            }),
            schedule: MessageField::some(every_schedule(30)),
            delivery: MessageField::some(nats_delivery(None)),
            message: MessageField::some(message(r#"{"kind":"heartbeat"}"#, [("x-kind", "heartbeat")])),
        }
    }

    fn every_schedule(every_sec: u64) -> v1::Schedule {
        v1::Schedule {
            kind: Some(
                v1::schedule::Every {
                    every: MessageField::some(Duration {
                        seconds: every_sec as i64,
                        ..Default::default()
                    }),
                }
                .into(),
            ),
        }
    }

    fn at_schedule(at: &str) -> v1::Schedule {
        v1::Schedule {
            kind: Some(
                v1::schedule::At {
                    at: MessageField::some(timestamp_from_str(at)),
                }
                .into(),
            ),
        }
    }

    fn cron_schedule(expr: &str, timezone: Option<&str>) -> v1::Schedule {
        v1::Schedule {
            kind: Some(
                v1::schedule::Cron {
                    expr: expr.to_string(),
                    timezone: timezone
                        .filter(|s| !s.is_empty())
                        .map(|tz| trogonai_proto::google::r#type::TimeZone {
                            id: tz.to_string(),
                            ..Default::default()
                        })
                        .map(MessageField::some)
                        .unwrap_or_else(MessageField::none),
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
                v1::schedule::RRule {
                    dtstart: MessageField::some(timestamp_from_str(dtstart)),
                    rrule: rrule.to_string(),
                    timezone: timezone
                        .filter(|s| !s.is_empty())
                        .map(|tz| trogonai_proto::google::r#type::TimeZone {
                            id: tz.to_string(),
                            ..Default::default()
                        })
                        .map(MessageField::some)
                        .unwrap_or_else(MessageField::none),
                    rdate: rdate.iter().map(|s| timestamp_from_str(s)).collect(),
                    exdate: exdate.iter().map(|s| timestamp_from_str(s)).collect(),
                }
                .into(),
            ),
        }
    }

    fn nats_delivery(source: Option<&str>) -> v1::Delivery {
        v1::Delivery {
            kind: Some(
                v1::delivery::NatsMessage {
                    subject: "agent.run".to_string(),
                    ttl: MessageField::some(Duration {
                        seconds: 15,
                        ..Default::default()
                    }),
                    source: source
                        .map(|subject| v1::delivery::nats_message::Source {
                            kind: Some(
                                v1::delivery::nats_message::LatestFromSubject {
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
            content: MessageField::some(trogonai_proto::content::v1alpha1::Content {
                content_type: "application/json".to_string(),
                data: content.as_bytes().to_vec(),
            }),
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

        // chrono::to_rfc3339() on a UTC DateTime produces "+00:00" offset
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
        job.status = MessageField::some(v1::ScheduleStatus {
            kind: Some(v1::schedule_status::Paused {}.into()),
        });

        let resolved = ResolvedSchedule::from_event("heartbeat", &job).unwrap();

        assert_eq!(resolved.id(), "heartbeat");
        assert!(!resolved.enabled());
        assert_eq!(resolved.route(), "agent.run");
        assert_eq!(
            resolved.schedule_body(),
            Bytes::from(b"{\x22kind\x22:\x22heartbeat\x22}".to_vec())
        );
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
