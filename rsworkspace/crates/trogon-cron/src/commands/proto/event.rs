use chrono::{DateTime, Utc};
use serde_json::{Map, Number, Value};
use trogon_cron_jobs_proto::v1;
use trogon_eventsourcing::{CanonicalEventCodec, EventCodec, EventData, EventType, RecordedEvent};

use crate::commands::event::{
    JobAdded, JobDetails, JobEvent, JobEventDelivery, JobEventSamplingSource, JobEventSchedule, JobEventStatus,
    JobPaused, JobRemoved, JobResumed, MessageContent, MessageEnvelope, MessageHeaders, MessageHeadersError,
};

pub use trogon_cron_jobs_proto::v1 as contract_v1;

pub type JobEventData = EventData;
pub type RecordedJobEvent = RecordedEvent;

pub const JOB_ADDED_EVENT_TYPE: &str = "trogon.cron.jobs.v1.JobAdded";
pub const JOB_PAUSED_EVENT_TYPE: &str = "trogon.cron.jobs.v1.JobPaused";
pub const JOB_RESUMED_EVENT_TYPE: &str = "trogon.cron.jobs.v1.JobResumed";
pub const JOB_REMOVED_EVENT_TYPE: &str = "trogon.cron.jobs.v1.JobRemoved";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JobEventCodec;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JobContractEventCodec;

#[derive(Debug)]
pub enum JobEventCodecError {
    ProtoJson(JobEventProtoJsonError),
    Proto(JobEventProtoError),
}

pub type JobContractEventCodecError = JobEventCodecError;

impl std::fmt::Display for JobEventCodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProtoJson(source) => write!(f, "{source}"),
            Self::Proto(source) => write!(f, "{source}"),
        }
    }
}

impl std::error::Error for JobEventCodecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ProtoJson(source) => Some(source),
            Self::Proto(source) => Some(source),
        }
    }
}

impl EventCodec<v1::JobEvent> for JobContractEventCodec {
    type Error = JobContractEventCodecError;

    fn encode(&self, value: &v1::JobEvent) -> Result<String, Self::Error> {
        encode_job_event_proto_json(value).map_err(JobEventCodecError::ProtoJson)
    }

    fn decode(&self, value: &str) -> Result<v1::JobEvent, Self::Error> {
        decode_job_event_proto_json(value).map_err(JobEventCodecError::ProtoJson)
    }
}

impl EventCodec<JobEvent> for JobEventCodec {
    type Error = JobEventCodecError;

    fn encode(&self, value: &JobEvent) -> Result<String, Self::Error> {
        encode_job_event_proto_json(&v1::JobEvent::from(value)).map_err(JobEventCodecError::ProtoJson)
    }

    fn decode(&self, value: &str) -> Result<JobEvent, Self::Error> {
        let event = decode_job_event_proto_json(value).map_err(JobEventCodecError::ProtoJson)?;
        event.try_into().map_err(JobEventCodecError::Proto)
    }
}

impl EventType for JobEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::JobAdded(..) => JOB_ADDED_EVENT_TYPE,
            Self::JobPaused(..) => JOB_PAUSED_EVENT_TYPE,
            Self::JobResumed(..) => JOB_RESUMED_EVENT_TYPE,
            Self::JobRemoved(..) => JOB_REMOVED_EVENT_TYPE,
        }
    }
}

impl CanonicalEventCodec for JobEvent {
    type Codec = JobEventCodec;

    fn canonical_codec() -> Self::Codec {
        JobEventCodec
    }
}

#[derive(Debug)]
pub enum JobEventProtoJsonError {
    Json(serde_json::Error),
    ExpectedObject {
        context: &'static str,
    },
    ExpectedArray {
        context: &'static str,
        field: &'static str,
    },
    InvalidField {
        context: &'static str,
        field: &'static str,
        expected: &'static str,
    },
    MissingOneof {
        context: &'static str,
    },
    MultipleOneofCases {
        context: &'static str,
    },
    InvalidEnum {
        context: &'static str,
        field: &'static str,
        value: String,
    },
    InvalidUnsignedInteger {
        context: &'static str,
        field: &'static str,
        value: String,
        source: std::num::ParseIntError,
    },
}

impl std::fmt::Display for JobEventProtoJsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(source) => write!(f, "{source}"),
            Self::ExpectedObject { context } => write!(f, "protobuf JSON {context} must be an object"),
            Self::ExpectedArray { context, field } => {
                write!(f, "protobuf JSON {context}.{field} must be an array")
            }
            Self::InvalidField {
                context,
                field,
                expected,
            } => write!(f, "protobuf JSON {context}.{field} must be {expected}"),
            Self::MissingOneof { context } => write!(f, "protobuf JSON {context} is missing its oneof case"),
            Self::MultipleOneofCases { context } => {
                write!(f, "protobuf JSON {context} contains multiple oneof cases")
            }
            Self::InvalidEnum { context, field, value } => {
                write!(f, "protobuf JSON {context}.{field} enum value '{value}' is invalid")
            }
            Self::InvalidUnsignedInteger {
                context,
                field,
                value,
                source,
            } => write!(
                f,
                "protobuf JSON {context}.{field} unsigned integer value '{value}' is invalid: {source}"
            ),
        }
    }
}

impl std::error::Error for JobEventProtoJsonError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(source) => Some(source),
            Self::InvalidUnsignedInteger { source, .. } => Some(source),
            Self::ExpectedObject { .. }
            | Self::ExpectedArray { .. }
            | Self::InvalidField { .. }
            | Self::MissingOneof { .. }
            | Self::MultipleOneofCases { .. }
            | Self::InvalidEnum { .. } => None,
        }
    }
}

fn encode_job_event_proto_json(event: &v1::JobEvent) -> Result<String, JobEventProtoJsonError> {
    serde_json::to_string(&job_event_to_json(event)?).map_err(JobEventProtoJsonError::Json)
}

fn decode_job_event_proto_json(value: &str) -> Result<v1::JobEvent, JobEventProtoJsonError> {
    let value = serde_json::from_str(value).map_err(JobEventProtoJsonError::Json)?;
    job_event_from_json(&value)
}

fn job_event_to_json(event: &v1::JobEvent) -> Result<Value, JobEventProtoJsonError> {
    let mut object = Map::new();
    match event.event() {
        v1::job_event::EventOneof::JobAdded(inner) => {
            object.insert("jobAdded".to_string(), job_added_to_json(&inner.to_owned())?);
        }
        v1::job_event::EventOneof::JobPaused(inner) => {
            object.insert("jobPaused".to_string(), id_event_to_json(inner.id()));
        }
        v1::job_event::EventOneof::JobResumed(inner) => {
            object.insert("jobResumed".to_string(), id_event_to_json(inner.id()));
        }
        v1::job_event::EventOneof::JobRemoved(inner) => {
            object.insert("jobRemoved".to_string(), id_event_to_json(inner.id()));
        }
        v1::job_event::EventOneof::not_set(_) | _ => {
            return Err(JobEventProtoJsonError::MissingOneof {
                context: "JobEvent.event",
            });
        }
    }
    Ok(Value::Object(object))
}

fn job_added_to_json(event: &v1::JobAdded) -> Result<Value, JobEventProtoJsonError> {
    let mut object = Map::new();
    object.insert("id".to_string(), Value::String(event.id().to_string()));
    if event.has_job() {
        object.insert("job".to_string(), job_details_to_json(&event.job().to_owned())?);
    }
    Ok(Value::Object(object))
}

fn id_event_to_json(id: impl ToString) -> Value {
    let mut object = Map::new();
    object.insert("id".to_string(), Value::String(id.to_string()));
    Value::Object(object)
}

fn job_details_to_json(job: &v1::JobDetails) -> Result<Value, JobEventProtoJsonError> {
    let mut object = Map::new();
    object.insert("status".to_string(), job_status_to_json(job.status()));
    if job.has_schedule() {
        object.insert(
            "schedule".to_string(),
            job_schedule_to_json(&job.schedule().to_owned())?,
        );
    }
    if job.has_delivery() {
        object.insert(
            "delivery".to_string(),
            job_delivery_to_json(&job.delivery().to_owned())?,
        );
    }
    if job.has_message() {
        object.insert("message".to_string(), job_message_to_json(&job.message().to_owned()));
    }
    Ok(Value::Object(object))
}

fn job_status_to_json(status: v1::JobStatus) -> Value {
    match i32::from(status) {
        0 => Value::String("JOB_STATUS_UNSPECIFIED".to_string()),
        1 => Value::String("JOB_STATUS_ENABLED".to_string()),
        2 => Value::String("JOB_STATUS_DISABLED".to_string()),
        other => Value::Number(Number::from(other)),
    }
}

fn job_schedule_to_json(schedule: &v1::JobSchedule) -> Result<Value, JobEventProtoJsonError> {
    let mut object = Map::new();
    match schedule.kind() {
        v1::job_schedule::KindOneof::At(inner) => {
            let mut at = Map::new();
            at.insert("at".to_string(), Value::String(inner.at().to_string()));
            object.insert("at".to_string(), Value::Object(at));
        }
        v1::job_schedule::KindOneof::Every(inner) => {
            let mut every = Map::new();
            every.insert("everySec".to_string(), Value::String(inner.every_sec().to_string()));
            object.insert("every".to_string(), Value::Object(every));
        }
        v1::job_schedule::KindOneof::Cron(inner) => {
            let mut cron = Map::new();
            cron.insert("expr".to_string(), Value::String(inner.expr().to_string()));
            if inner.has_timezone() {
                cron.insert("timezone".to_string(), Value::String(inner.timezone().to_string()));
            }
            object.insert("cron".to_string(), Value::Object(cron));
        }
        v1::job_schedule::KindOneof::not_set(_) | _ => {
            return Err(JobEventProtoJsonError::MissingOneof {
                context: "JobSchedule.kind",
            });
        }
    }
    Ok(Value::Object(object))
}

fn job_delivery_to_json(delivery: &v1::JobDelivery) -> Result<Value, JobEventProtoJsonError> {
    let mut object = Map::new();
    match delivery.kind() {
        v1::job_delivery::KindOneof::NatsEvent(inner) => {
            let mut nats_event = Map::new();
            nats_event.insert("route".to_string(), Value::String(inner.route().to_string()));
            if inner.has_ttl_sec() {
                nats_event.insert("ttlSec".to_string(), Value::String(inner.ttl_sec().to_string()));
            }
            if inner.has_source() {
                nats_event.insert(
                    "source".to_string(),
                    job_sampling_source_to_json(&inner.source().to_owned())?,
                );
            }
            object.insert("natsEvent".to_string(), Value::Object(nats_event));
        }
        v1::job_delivery::KindOneof::not_set(_) | _ => {
            return Err(JobEventProtoJsonError::MissingOneof {
                context: "JobDelivery.kind",
            });
        }
    }
    Ok(Value::Object(object))
}

fn job_sampling_source_to_json(source: &v1::JobSamplingSource) -> Result<Value, JobEventProtoJsonError> {
    let mut object = Map::new();
    match source.kind() {
        v1::job_sampling_source::KindOneof::LatestFromSubject(inner) => {
            let mut latest = Map::new();
            latest.insert("subject".to_string(), Value::String(inner.subject().to_string()));
            object.insert("latestFromSubject".to_string(), Value::Object(latest));
        }
        v1::job_sampling_source::KindOneof::not_set(_) | _ => {
            return Err(JobEventProtoJsonError::MissingOneof {
                context: "JobSamplingSource.kind",
            });
        }
    }
    Ok(Value::Object(object))
}

fn job_message_to_json(message: &v1::JobMessage) -> Value {
    let mut object = Map::new();
    object.insert("content".to_string(), Value::String(message.content().to_string()));

    let headers = message
        .headers()
        .iter()
        .map(|header| {
            let mut object = Map::new();
            object.insert("name".to_string(), Value::String(header.name().to_string()));
            object.insert("value".to_string(), Value::String(header.value().to_string()));
            Value::Object(object)
        })
        .collect::<Vec<_>>();
    if !headers.is_empty() {
        object.insert("headers".to_string(), Value::Array(headers));
    }

    Value::Object(object)
}

fn job_event_from_json(value: &Value) -> Result<v1::JobEvent, JobEventProtoJsonError> {
    let object = object(value, "JobEvent")?;
    let mut event = v1::JobEvent::new();
    match oneof_case(
        object,
        "JobEvent.event",
        &[
            ("jobAdded", "job_added"),
            ("jobPaused", "job_paused"),
            ("jobResumed", "job_resumed"),
            ("jobRemoved", "job_removed"),
        ],
    )? {
        Some((0, value)) => event.set_job_added(job_added_from_json(value)?),
        Some((1, value)) => event.set_job_paused(job_paused_from_json(value)?),
        Some((2, value)) => event.set_job_resumed(job_resumed_from_json(value)?),
        Some((3, value)) => event.set_job_removed(job_removed_from_json(value)?),
        Some(_) => unreachable!("oneof case index must map to the provided case list"),
        None => {
            return Err(JobEventProtoJsonError::MissingOneof {
                context: "JobEvent.event",
            });
        }
    }
    Ok(event)
}

fn job_added_from_json(value: &Value) -> Result<v1::JobAdded, JobEventProtoJsonError> {
    let object = object(value, "JobAdded")?;
    let id = string_field_or_default(object, "JobAdded", "id", "id")?;
    let mut event = v1::JobAdded::new();
    event.set_id(id.as_str());
    if let Some(job) = field(object, "job", "job") {
        event.set_job(job_details_from_json(job)?);
    }
    Ok(event)
}

fn job_paused_from_json(value: &Value) -> Result<v1::JobPaused, JobEventProtoJsonError> {
    let object = object(value, "JobPaused")?;
    let id = string_field_or_default(object, "JobPaused", "id", "id")?;
    let mut event = v1::JobPaused::new();
    event.set_id(id.as_str());
    Ok(event)
}

fn job_resumed_from_json(value: &Value) -> Result<v1::JobResumed, JobEventProtoJsonError> {
    let object = object(value, "JobResumed")?;
    let id = string_field_or_default(object, "JobResumed", "id", "id")?;
    let mut event = v1::JobResumed::new();
    event.set_id(id.as_str());
    Ok(event)
}

fn job_removed_from_json(value: &Value) -> Result<v1::JobRemoved, JobEventProtoJsonError> {
    let object = object(value, "JobRemoved")?;
    let id = string_field_or_default(object, "JobRemoved", "id", "id")?;
    let mut event = v1::JobRemoved::new();
    event.set_id(id.as_str());
    Ok(event)
}

fn job_details_from_json(value: &Value) -> Result<v1::JobDetails, JobEventProtoJsonError> {
    let object = object(value, "JobDetails")?;
    let mut job = v1::JobDetails::new();
    if let Some(status) = field(object, "status", "status") {
        job.set_status(job_status_from_json(status, "JobDetails", "status")?);
    }
    if let Some(schedule) = field(object, "schedule", "schedule") {
        job.set_schedule(job_schedule_from_json(schedule)?);
    }
    if let Some(delivery) = field(object, "delivery", "delivery") {
        job.set_delivery(job_delivery_from_json(delivery)?);
    }
    if let Some(message) = field(object, "message", "message") {
        job.set_message(job_message_from_json(message)?);
    }
    Ok(job)
}

fn job_status_from_json(
    value: &Value,
    context: &'static str,
    field: &'static str,
) -> Result<v1::JobStatus, JobEventProtoJsonError> {
    match value {
        Value::String(value) => match value.as_str() {
            "JOB_STATUS_UNSPECIFIED" => Ok(v1::JobStatus::Unspecified),
            "JOB_STATUS_ENABLED" => Ok(v1::JobStatus::Enabled),
            "JOB_STATUS_DISABLED" => Ok(v1::JobStatus::Disabled),
            other => Err(JobEventProtoJsonError::InvalidEnum {
                context,
                field,
                value: other.to_string(),
            }),
        },
        Value::Number(value) => value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .map(v1::JobStatus::from)
            .ok_or(JobEventProtoJsonError::InvalidField {
                context,
                field,
                expected: "a protobuf enum name or i32 value",
            }),
        _ => Err(JobEventProtoJsonError::InvalidField {
            context,
            field,
            expected: "a protobuf enum name or i32 value",
        }),
    }
}

fn job_schedule_from_json(value: &Value) -> Result<v1::JobSchedule, JobEventProtoJsonError> {
    let object = object(value, "JobSchedule")?;
    let mut schedule = v1::JobSchedule::new();
    match oneof_case(
        object,
        "JobSchedule.kind",
        &[("at", "at"), ("every", "every"), ("cron", "cron")],
    )? {
        Some((0, value)) => schedule.set_at(at_schedule_from_json(value)?),
        Some((1, value)) => schedule.set_every(every_schedule_from_json(value)?),
        Some((2, value)) => schedule.set_cron(cron_schedule_from_json(value)?),
        Some(_) => unreachable!("oneof case index must map to the provided case list"),
        None => {
            return Err(JobEventProtoJsonError::MissingOneof {
                context: "JobSchedule.kind",
            });
        }
    }
    Ok(schedule)
}

fn at_schedule_from_json(value: &Value) -> Result<v1::AtSchedule, JobEventProtoJsonError> {
    let object = object(value, "AtSchedule")?;
    let at = string_field_or_default(object, "AtSchedule", "at", "at")?;
    let mut schedule = v1::AtSchedule::new();
    schedule.set_at(at.as_str());
    Ok(schedule)
}

fn every_schedule_from_json(value: &Value) -> Result<v1::EverySchedule, JobEventProtoJsonError> {
    let object = object(value, "EverySchedule")?;
    let every_sec = u64_field_or_default(object, "EverySchedule", "everySec", "every_sec")?;
    let mut schedule = v1::EverySchedule::new();
    schedule.set_every_sec(every_sec);
    Ok(schedule)
}

fn cron_schedule_from_json(value: &Value) -> Result<v1::CronSchedule, JobEventProtoJsonError> {
    let object = object(value, "CronSchedule")?;
    let expr = string_field_or_default(object, "CronSchedule", "expr", "expr")?;
    let mut schedule = v1::CronSchedule::new();
    schedule.set_expr(expr.as_str());
    if let Some(timezone) = optional_string_field(object, "CronSchedule", "timezone", "timezone")? {
        schedule.set_timezone(timezone.as_str());
    }
    Ok(schedule)
}

fn job_delivery_from_json(value: &Value) -> Result<v1::JobDelivery, JobEventProtoJsonError> {
    let object = object(value, "JobDelivery")?;
    let mut delivery = v1::JobDelivery::new();
    match oneof_case(object, "JobDelivery.kind", &[("natsEvent", "nats_event")])? {
        Some((0, value)) => delivery.set_nats_event(nats_event_delivery_from_json(value)?),
        Some(_) => unreachable!("oneof case index must map to the provided case list"),
        None => {
            return Err(JobEventProtoJsonError::MissingOneof {
                context: "JobDelivery.kind",
            });
        }
    }
    Ok(delivery)
}

fn nats_event_delivery_from_json(value: &Value) -> Result<v1::NatsEventDelivery, JobEventProtoJsonError> {
    let object = object(value, "NatsEventDelivery")?;
    let route = string_field_or_default(object, "NatsEventDelivery", "route", "route")?;
    let mut delivery = v1::NatsEventDelivery::new();
    delivery.set_route(route.as_str());
    if let Some(ttl_sec) = optional_u64_field(object, "NatsEventDelivery", "ttlSec", "ttl_sec")? {
        delivery.set_ttl_sec(ttl_sec);
    }
    if let Some(source) = field(object, "source", "source") {
        delivery.set_source(job_sampling_source_from_json(source)?);
    }
    Ok(delivery)
}

fn job_sampling_source_from_json(value: &Value) -> Result<v1::JobSamplingSource, JobEventProtoJsonError> {
    let object = object(value, "JobSamplingSource")?;
    let mut source = v1::JobSamplingSource::new();
    match oneof_case(
        object,
        "JobSamplingSource.kind",
        &[("latestFromSubject", "latest_from_subject")],
    )? {
        Some((0, value)) => source.set_latest_from_subject(latest_from_subject_sampling_from_json(value)?),
        Some(_) => unreachable!("oneof case index must map to the provided case list"),
        None => {
            return Err(JobEventProtoJsonError::MissingOneof {
                context: "JobSamplingSource.kind",
            });
        }
    }
    Ok(source)
}

fn latest_from_subject_sampling_from_json(
    value: &Value,
) -> Result<v1::LatestFromSubjectSampling, JobEventProtoJsonError> {
    let object = object(value, "LatestFromSubjectSampling")?;
    let subject = string_field_or_default(object, "LatestFromSubjectSampling", "subject", "subject")?;
    let mut source = v1::LatestFromSubjectSampling::new();
    source.set_subject(subject.as_str());
    Ok(source)
}

fn job_message_from_json(value: &Value) -> Result<v1::JobMessage, JobEventProtoJsonError> {
    let object = object(value, "JobMessage")?;
    let content = string_field_or_default(object, "JobMessage", "content", "content")?;
    let mut message = v1::JobMessage::new();
    message.set_content(content.as_str());

    if let Some(headers) = field(object, "headers", "headers") {
        for header in array(headers, "JobMessage", "headers")? {
            message.headers_mut().push(header_from_json(header)?);
        }
    }

    Ok(message)
}

fn header_from_json(value: &Value) -> Result<v1::Header, JobEventProtoJsonError> {
    let object = object(value, "Header")?;
    let name = string_field_or_default(object, "Header", "name", "name")?;
    let value = string_field_or_default(object, "Header", "value", "value")?;
    let mut header = v1::Header::new();
    header.set_name(name.as_str());
    header.set_value(value.as_str());
    Ok(header)
}

fn object<'a>(value: &'a Value, context: &'static str) -> Result<&'a Map<String, Value>, JobEventProtoJsonError> {
    value
        .as_object()
        .ok_or(JobEventProtoJsonError::ExpectedObject { context })
}

fn array<'a>(
    value: &'a Value,
    context: &'static str,
    field: &'static str,
) -> Result<&'a Vec<Value>, JobEventProtoJsonError> {
    value
        .as_array()
        .ok_or(JobEventProtoJsonError::ExpectedArray { context, field })
}

fn field<'a>(object: &'a Map<String, Value>, camel: &'static str, proto: &'static str) -> Option<&'a Value> {
    object.get(camel).or_else(|| object.get(proto))
}

fn oneof_case<'a>(
    object: &'a Map<String, Value>,
    context: &'static str,
    cases: &[(&'static str, &'static str)],
) -> Result<Option<(usize, &'a Value)>, JobEventProtoJsonError> {
    let mut found = None;
    for (index, (camel, proto)) in cases.iter().enumerate() {
        if let Some(value) = object.get(*camel) {
            if found.is_some() {
                return Err(JobEventProtoJsonError::MultipleOneofCases { context });
            }
            found = Some((index, value));
        }
        if camel != proto
            && let Some(value) = object.get(*proto)
        {
            if found.is_some() {
                return Err(JobEventProtoJsonError::MultipleOneofCases { context });
            }
            found = Some((index, value));
        }
    }
    Ok(found)
}

fn string_field_or_default(
    object: &Map<String, Value>,
    context: &'static str,
    camel: &'static str,
    proto: &'static str,
) -> Result<String, JobEventProtoJsonError> {
    optional_string_field(object, context, camel, proto).map(|value| value.unwrap_or_default())
}

fn optional_string_field(
    object: &Map<String, Value>,
    context: &'static str,
    camel: &'static str,
    proto: &'static str,
) -> Result<Option<String>, JobEventProtoJsonError> {
    field(object, camel, proto)
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or(JobEventProtoJsonError::InvalidField {
                    context,
                    field: camel,
                    expected: "a string",
                })
        })
        .transpose()
}

fn u64_field_or_default(
    object: &Map<String, Value>,
    context: &'static str,
    camel: &'static str,
    proto: &'static str,
) -> Result<u64, JobEventProtoJsonError> {
    optional_u64_field(object, context, camel, proto).map(|value| value.unwrap_or_default())
}

fn optional_u64_field(
    object: &Map<String, Value>,
    context: &'static str,
    camel: &'static str,
    proto: &'static str,
) -> Result<Option<u64>, JobEventProtoJsonError> {
    field(object, camel, proto)
        .map(|value| u64_from_json(value, context, camel))
        .transpose()
}

fn u64_from_json(value: &Value, context: &'static str, field: &'static str) -> Result<u64, JobEventProtoJsonError> {
    match value {
        Value::String(value) => value
            .parse()
            .map_err(|source| JobEventProtoJsonError::InvalidUnsignedInteger {
                context,
                field,
                value: value.clone(),
                source,
            }),
        Value::Number(value) => value.as_u64().ok_or(JobEventProtoJsonError::InvalidField {
            context,
            field,
            expected: "a non-negative integer",
        }),
        _ => Err(JobEventProtoJsonError::InvalidField {
            context,
            field,
            expected: "a protobuf uint64 string or non-negative integer",
        }),
    }
}

#[derive(Debug)]
pub enum JobEventProtoError {
    MissingEvent,
    MissingJobDetails,
    MissingSchedule,
    MissingDelivery,
    MissingMessage,
    MissingScheduleKind,
    MissingDeliveryKind,
    MissingSamplingSourceKind,
    UnknownJobStatus { value: i32 },
    InvalidTimestamp { value: String, source: chrono::ParseError },
    InvalidHeaders(MessageHeadersError),
}

impl std::fmt::Display for JobEventProtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingEvent => f.write_str("protobuf job event is missing its oneof case"),
            Self::MissingJobDetails => f.write_str("protobuf job_added is missing job details"),
            Self::MissingSchedule => f.write_str("protobuf job details are missing schedule"),
            Self::MissingDelivery => f.write_str("protobuf job details are missing delivery"),
            Self::MissingMessage => f.write_str("protobuf job details are missing message"),
            Self::MissingScheduleKind => f.write_str("protobuf job schedule is missing its oneof case"),
            Self::MissingDeliveryKind => f.write_str("protobuf job delivery is missing its oneof case"),
            Self::MissingSamplingSourceKind => f.write_str("protobuf sampling source is missing its oneof case"),
            Self::UnknownJobStatus { value } => write!(f, "protobuf job status '{value}' is unknown"),
            Self::InvalidTimestamp { value, source } => {
                write!(f, "protobuf timestamp '{value}' is invalid: {source}")
            }
            Self::InvalidHeaders(source) => write!(f, "protobuf headers are invalid: {source}"),
        }
    }
}

impl std::error::Error for JobEventProtoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidTimestamp { source, .. } => Some(source),
            Self::InvalidHeaders(source) => Some(source),
            Self::MissingEvent
            | Self::MissingJobDetails
            | Self::MissingSchedule
            | Self::MissingDelivery
            | Self::MissingMessage
            | Self::MissingScheduleKind
            | Self::MissingDeliveryKind
            | Self::MissingSamplingSourceKind
            | Self::UnknownJobStatus { .. } => None,
        }
    }
}

impl From<JobEvent> for v1::JobEvent {
    fn from(value: JobEvent) -> Self {
        Self::from(&value)
    }
}

impl From<&JobEvent> for v1::JobEvent {
    fn from(value: &JobEvent) -> Self {
        let mut event = v1::JobEvent::new();
        match value {
            JobEvent::JobAdded(inner) => event.set_job_added(v1::JobAdded::from(inner)),
            JobEvent::JobPaused(inner) => event.set_job_paused(v1::JobPaused::from(inner)),
            JobEvent::JobResumed(inner) => event.set_job_resumed(v1::JobResumed::from(inner)),
            JobEvent::JobRemoved(inner) => event.set_job_removed(v1::JobRemoved::from(inner)),
        }
        event
    }
}

impl TryFrom<v1::JobEvent> for JobEvent {
    type Error = JobEventProtoError;

    fn try_from(value: v1::JobEvent) -> Result<Self, Self::Error> {
        match value.event() {
            v1::job_event::EventOneof::JobAdded(inner) => Ok(Self::JobAdded(inner.to_owned().try_into()?)),
            v1::job_event::EventOneof::JobPaused(inner) => Ok(Self::JobPaused(inner.to_owned().try_into()?)),
            v1::job_event::EventOneof::JobResumed(inner) => Ok(Self::JobResumed(inner.to_owned().try_into()?)),
            v1::job_event::EventOneof::JobRemoved(inner) => Ok(Self::JobRemoved(inner.to_owned().try_into()?)),
            v1::job_event::EventOneof::not_set(_) | _ => Err(JobEventProtoError::MissingEvent),
        }
    }
}

pub fn contract_event_stream_id(event: &v1::JobEvent) -> Result<String, JobEventProtoError> {
    match event.event() {
        v1::job_event::EventOneof::JobAdded(inner) => Ok(inner.id().to_string()),
        v1::job_event::EventOneof::JobPaused(inner) => Ok(inner.id().to_string()),
        v1::job_event::EventOneof::JobResumed(inner) => Ok(inner.id().to_string()),
        v1::job_event::EventOneof::JobRemoved(inner) => Ok(inner.id().to_string()),
        v1::job_event::EventOneof::not_set(_) | _ => Err(JobEventProtoError::MissingEvent),
    }
}

impl From<&JobAdded> for v1::JobAdded {
    fn from(value: &JobAdded) -> Self {
        let mut event = v1::JobAdded::new();
        event.set_id(value.id.as_str());
        event.set_job(v1::JobDetails::from(&value.job));
        event
    }
}

impl TryFrom<v1::JobAdded> for JobAdded {
    type Error = JobEventProtoError;

    fn try_from(value: v1::JobAdded) -> Result<Self, Self::Error> {
        if !value.has_job() {
            return Err(JobEventProtoError::MissingJobDetails);
        }
        Ok(Self {
            id: value.id().to_string(),
            job: value.job().to_owned().try_into()?,
        })
    }
}

impl From<&JobPaused> for v1::JobPaused {
    fn from(value: &JobPaused) -> Self {
        let mut event = v1::JobPaused::new();
        event.set_id(value.id.as_str());
        event
    }
}

impl TryFrom<v1::JobPaused> for JobPaused {
    type Error = JobEventProtoError;

    fn try_from(value: v1::JobPaused) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id().to_string(),
        })
    }
}

impl From<&JobResumed> for v1::JobResumed {
    fn from(value: &JobResumed) -> Self {
        let mut event = v1::JobResumed::new();
        event.set_id(value.id.as_str());
        event
    }
}

impl TryFrom<v1::JobResumed> for JobResumed {
    type Error = JobEventProtoError;

    fn try_from(value: v1::JobResumed) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id().to_string(),
        })
    }
}

impl From<&JobRemoved> for v1::JobRemoved {
    fn from(value: &JobRemoved) -> Self {
        let mut event = v1::JobRemoved::new();
        event.set_id(value.id.as_str());
        event
    }
}

impl TryFrom<v1::JobRemoved> for JobRemoved {
    type Error = JobEventProtoError;

    fn try_from(value: v1::JobRemoved) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id().to_string(),
        })
    }
}

impl From<&JobDetails> for v1::JobDetails {
    fn from(value: &JobDetails) -> Self {
        let mut job = v1::JobDetails::new();
        job.set_status(v1::JobStatus::from(value.status));
        job.set_schedule(v1::JobSchedule::from(&value.schedule));
        job.set_delivery(v1::JobDelivery::from(&value.delivery));
        job.set_message(v1::JobMessage::from(&value.message));
        job
    }
}

impl TryFrom<v1::JobDetails> for JobDetails {
    type Error = JobEventProtoError;

    fn try_from(value: v1::JobDetails) -> Result<Self, Self::Error> {
        if !value.has_schedule() {
            return Err(JobEventProtoError::MissingSchedule);
        }
        if !value.has_delivery() {
            return Err(JobEventProtoError::MissingDelivery);
        }
        if !value.has_message() {
            return Err(JobEventProtoError::MissingMessage);
        }
        Ok(Self {
            status: value.status().try_into()?,
            schedule: value.schedule().to_owned().try_into()?,
            delivery: value.delivery().to_owned().try_into()?,
            message: value.message().to_owned().try_into()?,
        })
    }
}

impl From<JobEventStatus> for v1::JobStatus {
    fn from(value: JobEventStatus) -> Self {
        match value {
            JobEventStatus::Enabled => Self::Enabled,
            JobEventStatus::Disabled => Self::Disabled,
        }
    }
}

impl TryFrom<v1::JobStatus> for JobEventStatus {
    type Error = JobEventProtoError;

    fn try_from(value: v1::JobStatus) -> Result<Self, Self::Error> {
        match i32::from(value) {
            1 => Ok(Self::Enabled),
            2 => Ok(Self::Disabled),
            other => Err(JobEventProtoError::UnknownJobStatus { value: other }),
        }
    }
}

impl From<&JobEventSchedule> for v1::JobSchedule {
    fn from(value: &JobEventSchedule) -> Self {
        let mut schedule = v1::JobSchedule::new();
        match value {
            JobEventSchedule::At { at } => {
                let mut inner = v1::AtSchedule::new();
                inner.set_at(at.to_rfc3339());
                schedule.set_at(inner);
            }
            JobEventSchedule::Every { every_sec } => {
                let mut inner = v1::EverySchedule::new();
                inner.set_every_sec(*every_sec);
                schedule.set_every(inner);
            }
            JobEventSchedule::Cron { expr, timezone } => {
                let mut inner = v1::CronSchedule::new();
                inner.set_expr(expr.as_str());
                if let Some(timezone) = timezone {
                    inner.set_timezone(timezone.as_str());
                }
                schedule.set_cron(inner);
            }
        }
        schedule
    }
}

impl TryFrom<v1::JobSchedule> for JobEventSchedule {
    type Error = JobEventProtoError;

    fn try_from(value: v1::JobSchedule) -> Result<Self, Self::Error> {
        match value.kind() {
            v1::job_schedule::KindOneof::At(inner) => {
                let at = inner.at().to_string();
                let parsed = DateTime::parse_from_rfc3339(&at)
                    .map_err(|source| JobEventProtoError::InvalidTimestamp { value: at, source })?
                    .with_timezone(&Utc);
                Ok(Self::At { at: parsed })
            }
            v1::job_schedule::KindOneof::Every(inner) => Ok(Self::Every {
                every_sec: inner.every_sec(),
            }),
            v1::job_schedule::KindOneof::Cron(inner) => Ok(Self::Cron {
                expr: inner.expr().to_string(),
                timezone: inner.has_timezone().then(|| inner.timezone().to_string()),
            }),
            v1::job_schedule::KindOneof::not_set(_) | _ => Err(JobEventProtoError::MissingScheduleKind),
        }
    }
}

impl From<&JobEventDelivery> for v1::JobDelivery {
    fn from(value: &JobEventDelivery) -> Self {
        let mut delivery = v1::JobDelivery::new();
        match value {
            JobEventDelivery::NatsEvent { route, ttl_sec, source } => {
                let mut inner = v1::NatsEventDelivery::new();
                inner.set_route(route.as_str());
                if let Some(ttl_sec) = ttl_sec {
                    inner.set_ttl_sec(*ttl_sec);
                }
                if let Some(source) = source {
                    inner.set_source(v1::JobSamplingSource::from(source));
                }
                delivery.set_nats_event(inner);
            }
        }
        delivery
    }
}

impl TryFrom<v1::JobDelivery> for JobEventDelivery {
    type Error = JobEventProtoError;

    fn try_from(value: v1::JobDelivery) -> Result<Self, Self::Error> {
        match value.kind() {
            v1::job_delivery::KindOneof::NatsEvent(inner) => Ok(Self::NatsEvent {
                route: inner.route().to_string(),
                ttl_sec: inner.has_ttl_sec().then(|| inner.ttl_sec()),
                source: inner
                    .has_source()
                    .then(|| inner.source().to_owned().try_into())
                    .transpose()?,
            }),
            v1::job_delivery::KindOneof::not_set(_) | _ => Err(JobEventProtoError::MissingDeliveryKind),
        }
    }
}

impl From<&JobEventSamplingSource> for v1::JobSamplingSource {
    fn from(value: &JobEventSamplingSource) -> Self {
        let mut source = v1::JobSamplingSource::new();
        match value {
            JobEventSamplingSource::LatestFromSubject { subject } => {
                let mut inner = v1::LatestFromSubjectSampling::new();
                inner.set_subject(subject.as_str());
                source.set_latest_from_subject(inner);
            }
        }
        source
    }
}

impl TryFrom<v1::JobSamplingSource> for JobEventSamplingSource {
    type Error = JobEventProtoError;

    fn try_from(value: v1::JobSamplingSource) -> Result<Self, Self::Error> {
        match value.kind() {
            v1::job_sampling_source::KindOneof::LatestFromSubject(inner) => Ok(Self::LatestFromSubject {
                subject: inner.subject().to_string(),
            }),
            v1::job_sampling_source::KindOneof::not_set(_) | _ => Err(JobEventProtoError::MissingSamplingSourceKind),
        }
    }
}

impl From<&MessageEnvelope> for v1::JobMessage {
    fn from(value: &MessageEnvelope) -> Self {
        let mut message = v1::JobMessage::new();
        message.set_content(value.content.as_str());
        for (name, val) in value.headers.as_slice() {
            let mut header = v1::Header::new();
            header.set_name(name.as_str());
            header.set_value(val.as_str());
            message.headers_mut().push(header);
        }
        message
    }
}

impl TryFrom<v1::JobMessage> for MessageEnvelope {
    type Error = JobEventProtoError;

    fn try_from(value: v1::JobMessage) -> Result<Self, Self::Error> {
        let headers = value
            .headers()
            .iter()
            .map(|header| (header.name().to_string(), header.value().to_string()))
            .collect::<Vec<_>>();

        Ok(Self {
            content: MessageContent::new(value.content().to_string()),
            headers: MessageHeaders::new(headers).map_err(JobEventProtoError::InvalidHeaders)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_data_and_recorded_event_helpers_work() {
        let event = JobEventData::new_with_codec(
            "cleanup",
            &JobEventCodec,
            JobEvent::JobRemoved(JobRemoved {
                id: "cleanup".to_string(),
            }),
        )
        .unwrap();
        assert_eq!(event.stream_id(), "cleanup");
        assert_eq!(event.event_type, JOB_REMOVED_EVENT_TYPE);
        assert_eq!(
            serde_json::from_str::<Value>(&event.data).unwrap(),
            serde_json::json!({
                "jobRemoved": {
                    "id": "cleanup"
                }
            })
        );
        assert_eq!(
            event.subject_with_prefix("cron.events.jobs."),
            "cron.events.jobs.cleanup"
        );

        let payload = serde_json::to_vec(&event).unwrap();
        let decoded = JobEventData::decode(&payload).unwrap();
        assert_eq!(decoded, event);
        assert_eq!(
            decoded.decode_data_with(&JobEventCodec).unwrap(),
            JobEvent::JobRemoved(JobRemoved {
                id: "cleanup".to_string()
            })
        );

        let recorded = event.record(
            "cron.jobs.events.cleanup",
            None,
            Some(9),
            chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
        );
        assert_eq!(recorded.stream_id(), "cleanup");
        assert_eq!(recorded.recorded_stream_id, "cron.jobs.events.cleanup");
        assert_eq!(recorded.log_position, Some(9));
        assert_eq!(
            recorded.subject_with_prefix("cron.events.jobs."),
            "cron.events.jobs.cleanup"
        );
        let recorded_payload = serde_json::to_vec(&recorded).unwrap();
        let decoded = RecordedJobEvent::decode(&recorded_payload).unwrap();
        assert_eq!(decoded, recorded);
        assert_eq!(
            decoded.decode_data_with(&JobEventCodec).unwrap(),
            JobEvent::JobRemoved(JobRemoved {
                id: "cleanup".to_string()
            })
        );
    }

    #[test]
    fn invalid_payload_fails_decode() {
        assert!(JobEventData::decode(br#"not-json"#).is_err());
        assert!(RecordedJobEvent::decode(br#"not-json"#).is_err());
    }

    #[test]
    fn job_added_round_trips_through_contract() {
        let event = JobEvent::JobAdded(JobAdded {
            id: "backup".to_string(),
            job: JobDetails {
                status: JobEventStatus::Enabled,
                schedule: JobEventSchedule::Cron {
                    expr: "0 * * * * *".to_string(),
                    timezone: Some("UTC".to_string()),
                },
                delivery: JobEventDelivery::NatsEvent {
                    route: "ops.backup".to_string(),
                    ttl_sec: Some(30),
                    source: Some(JobEventSamplingSource::LatestFromSubject {
                        subject: "events.backup".to_string(),
                    }),
                },
                message: MessageEnvelope {
                    content: MessageContent::from_static("hello"),
                    headers: MessageHeaders::new([("x-kind", "backup")]).unwrap(),
                },
            },
        });

        let proto = v1::JobEvent::from(&event);
        let decoded = JobEvent::try_from(proto).unwrap();
        let encoded = JobEventCodec.encode(&event).unwrap();

        assert_eq!(decoded, event);
        assert_eq!(
            serde_json::from_str::<Value>(&encoded).unwrap(),
            serde_json::json!({
                "jobAdded": {
                    "id": "backup",
                    "job": {
                        "status": "JOB_STATUS_ENABLED",
                        "schedule": {
                            "cron": {
                                "expr": "0 * * * * *",
                                "timezone": "UTC"
                            }
                        },
                        "delivery": {
                            "natsEvent": {
                                "route": "ops.backup",
                                "ttlSec": "30",
                                "source": {
                                    "latestFromSubject": {
                                        "subject": "events.backup"
                                    }
                                }
                            }
                        },
                        "message": {
                            "content": "hello",
                            "headers": [
                                {
                                    "name": "x-kind",
                                    "value": "backup"
                                }
                            ]
                        }
                    }
                }
            })
        );
        assert_eq!(JobEventCodec.decode(&encoded).unwrap(), event);
    }

    #[test]
    fn unknown_status_is_rejected() {
        let mut details = v1::JobDetails::new();
        details.set_status(v1::JobStatus::from(99));
        details.set_schedule(v1::JobSchedule::from(&JobEventSchedule::Every { every_sec: 30 }));
        details.set_delivery(v1::JobDelivery::from(&JobEventDelivery::NatsEvent {
            route: "ops.backup".to_string(),
            ttl_sec: None,
            source: None,
        }));
        details.set_message(v1::JobMessage::from(&MessageEnvelope {
            content: MessageContent::from_static("hello"),
            headers: MessageHeaders::default(),
        }));

        let error = JobDetails::try_from(details).unwrap_err();
        assert!(matches!(error, JobEventProtoError::UnknownJobStatus { value: 99 }));
    }
}
