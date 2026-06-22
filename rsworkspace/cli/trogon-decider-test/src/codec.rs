//! Typed JSON ↔ proto encoding for decider-test YAML suites.

use anyhow::{Context, Result, bail};
use buffa::{Message as _, MessageField, MessageName as _};
use trogon_decider_wit::host::{self, CommandEnvelope};
use trogonai_proto::content::v1alpha1 as content_v1alpha1;
use trogonai_proto::example::{TURN_ON_TYPE_URL, v1 as light_v1};
use trogonai_proto::scheduler::schedules::{
    CREATE_SCHEDULE_TYPE_URL, PAUSE_SCHEDULE_TYPE_URL, REMOVE_SCHEDULE_TYPE_URL, RESUME_SCHEDULE_TYPE_URL,
    v1 as schedules_v1,
};

pub fn json_any_to_command(value: &serde_json::Value) -> Result<CommandEnvelope> {
    let type_url = any_type_url(value)?;
    match type_url.as_str() {
        TURN_ON_TYPE_URL => {
            let turn_on = parse_turn_on(value)?;
            Ok(CommandEnvelope {
                type_: type_url,
                payload: turn_on.encode_to_vec(),
            })
        }
        CREATE_SCHEDULE_TYPE_URL => Ok(CommandEnvelope {
            type_: type_url,
            payload: parse_create_schedule_command(value)?.encode_to_vec(),
        }),
        PAUSE_SCHEDULE_TYPE_URL => Ok(CommandEnvelope {
            type_: type_url,
            payload: parse_schedule_id_command::<schedules_v1::PauseScheduleCommand>(value)?.encode_to_vec(),
        }),
        REMOVE_SCHEDULE_TYPE_URL => Ok(CommandEnvelope {
            type_: type_url,
            payload: parse_schedule_id_command::<schedules_v1::RemoveScheduleCommand>(value)?.encode_to_vec(),
        }),
        RESUME_SCHEDULE_TYPE_URL => Ok(CommandEnvelope {
            type_: type_url,
            payload: parse_schedule_id_command::<schedules_v1::ResumeScheduleCommand>(value)?.encode_to_vec(),
        }),
        other => bail!("unsupported command type '{other}'"),
    }
}

pub fn json_any_to_envelope(value: &serde_json::Value) -> Result<host::AnyEnvelope> {
    let type_url = any_type_url(value)?;
    let message_name = type_url_to_message_name(&type_url);
    match message_name.as_str() {
        light_v1::LightTurnedOn::FULL_NAME => {
            let event = parse_light_turned_on(value)?;
            Ok(host::AnyEnvelope {
                type_: light_v1::LightTurnedOn::FULL_NAME.to_string(),
                payload: event.encode_to_vec(),
            })
        }
        schedules_v1::ScheduleCreated::FULL_NAME => Ok(host::AnyEnvelope {
            type_: schedules_v1::ScheduleCreated::FULL_NAME.to_string(),
            payload: parse_schedule_created(value)?.encode_to_vec(),
        }),
        schedules_v1::SchedulePaused::FULL_NAME => Ok(host::AnyEnvelope {
            type_: schedules_v1::SchedulePaused::FULL_NAME.to_string(),
            payload: parse_schedule_paused(value)?.encode_to_vec(),
        }),
        schedules_v1::ScheduleResumed::FULL_NAME => Ok(host::AnyEnvelope {
            type_: schedules_v1::ScheduleResumed::FULL_NAME.to_string(),
            payload: parse_schedule_resumed(value)?.encode_to_vec(),
        }),
        schedules_v1::ScheduleRemoved::FULL_NAME => Ok(host::AnyEnvelope {
            type_: schedules_v1::ScheduleRemoved::FULL_NAME.to_string(),
            payload: parse_schedule_removed(value)?.encode_to_vec(),
        }),
        other => bail!("unsupported event type '{other}'"),
    }
}

pub fn any_type_url(value: &serde_json::Value) -> Result<String> {
    value
        .get("@type")
        .and_then(serde_json::Value::as_str)
        .map(normalize_type_url)
        .context("payload missing @type")
}

pub fn normalize_type_url(type_url: &str) -> String {
    if type_url.starts_with("type.googleapis.com/") {
        type_url.to_string()
    } else {
        format!("type.googleapis.com/{type_url}")
    }
}

fn type_url_to_message_name(type_url: &str) -> String {
    type_url
        .trim_start_matches("type.googleapis.com/")
        .trim_start_matches('/')
        .to_string()
}

fn parse_turn_on(value: &serde_json::Value) -> Result<light_v1::TurnOn> {
    Ok(light_v1::TurnOn {
        light_id: required_string(value, "light_id")?,
    })
}

fn parse_light_turned_on(value: &serde_json::Value) -> Result<light_v1::LightTurnedOn> {
    Ok(light_v1::LightTurnedOn {
        light_id: required_string(value, "light_id")?,
        turn_on_count: value
            .get("turn_on_count")
            .and_then(serde_json::Value::as_u64)
            .context("LightTurnedOn missing turn_on_count")?,
    })
}

fn parse_create_schedule_command(value: &serde_json::Value) -> Result<schedules_v1::CreateScheduleCommand> {
    Ok(schedules_v1::CreateScheduleCommand {
        schedule_id: required_string(value, "schedule_id")?,
        status: MessageField::some(parse_schedule_status(value.get("status"))?),
        schedule: MessageField::some(parse_schedule(value.get("schedule"))?),
        delivery: MessageField::some(parse_delivery(value.get("delivery"))?),
        message: MessageField::some(parse_message(value.get("message"))?),
    })
}

fn parse_schedule_id_command<P>(value: &serde_json::Value) -> Result<P>
where
    P: ScheduleIdCommand,
{
    P::from_schedule_id(required_string(value, "schedule_id")?)
}

trait ScheduleIdCommand: buffa::Message {
    fn from_schedule_id(schedule_id: String) -> Result<Self>;
}

impl ScheduleIdCommand for schedules_v1::PauseScheduleCommand {
    fn from_schedule_id(schedule_id: String) -> Result<Self> {
        Ok(Self { schedule_id })
    }
}

impl ScheduleIdCommand for schedules_v1::RemoveScheduleCommand {
    fn from_schedule_id(schedule_id: String) -> Result<Self> {
        Ok(Self { schedule_id })
    }
}

impl ScheduleIdCommand for schedules_v1::ResumeScheduleCommand {
    fn from_schedule_id(schedule_id: String) -> Result<Self> {
        Ok(Self { schedule_id })
    }
}

fn parse_schedule_created(value: &serde_json::Value) -> Result<schedules_v1::ScheduleCreated> {
    Ok(schedules_v1::ScheduleCreated {
        schedule_id: required_string(value, "schedule_id")?,
        status: MessageField::some(parse_schedule_status(value.get("status"))?),
        schedule: MessageField::some(parse_schedule(value.get("schedule"))?),
        delivery: MessageField::some(parse_delivery(value.get("delivery"))?),
        message: MessageField::some(parse_message(value.get("message"))?),
    })
}

fn parse_schedule_paused(value: &serde_json::Value) -> Result<schedules_v1::SchedulePaused> {
    Ok(schedules_v1::SchedulePaused {
        schedule_id: required_string(value, "schedule_id")?,
    })
}

fn parse_schedule_resumed(value: &serde_json::Value) -> Result<schedules_v1::ScheduleResumed> {
    Ok(schedules_v1::ScheduleResumed {
        schedule_id: required_string(value, "schedule_id")?,
    })
}

fn parse_schedule_removed(value: &serde_json::Value) -> Result<schedules_v1::ScheduleRemoved> {
    Ok(schedules_v1::ScheduleRemoved {
        schedule_id: required_string(value, "schedule_id")?,
    })
}

fn parse_schedule_status(value: Option<&serde_json::Value>) -> Result<schedules_v1::ScheduleStatus> {
    let value = value.context("missing schedule status")?;
    if value.get("scheduled").is_some() {
        return Ok(schedules_v1::ScheduleStatus {
            kind: Some(schedules_v1::schedule_status::Scheduled {}.into()),
        });
    }
    if value.get("paused").is_some() {
        return Ok(schedules_v1::ScheduleStatus {
            kind: Some(schedules_v1::schedule_status::Paused {}.into()),
        });
    }
    bail!("schedule status must contain scheduled or paused")
}

fn parse_schedule(value: Option<&serde_json::Value>) -> Result<schedules_v1::Schedule> {
    let value = value.context("missing schedule")?;
    if let Some(every) = value.get("every") {
        let seconds = every
            .get("seconds")
            .and_then(serde_json::Value::as_i64)
            .context("every schedule missing seconds")?;
        return Ok(schedules_v1::Schedule {
            kind: Some(
                schedules_v1::schedule::Every {
                    every: MessageField::some(buffa_types::google::protobuf::Duration {
                        seconds,
                        nanos: every.get("nanos").and_then(serde_json::Value::as_i64).unwrap_or(0) as i32,
                        ..buffa_types::google::protobuf::Duration::default()
                    }),
                }
                .into(),
            ),
        });
    }
    bail!("unsupported schedule shape in test YAML")
}

fn parse_delivery(value: Option<&serde_json::Value>) -> Result<schedules_v1::Delivery> {
    let value = value.context("missing delivery")?;
    let subject = value
        .get("subject")
        .or_else(|| value.get("nats_message").and_then(|inner| inner.get("subject")))
        .and_then(serde_json::Value::as_str)
        .context("delivery missing subject")?;
    Ok(schedules_v1::Delivery {
        kind: Some(
            schedules_v1::delivery::NatsMessage {
                subject: subject.to_string(),
                ttl: MessageField::none(),
                source: MessageField::none(),
            }
            .into(),
        ),
    })
}

fn parse_message(value: Option<&serde_json::Value>) -> Result<schedules_v1::Message> {
    let value = value.context("missing message")?;
    let content = value.get("content").context("message missing content")?;
    let data = content
        .get("data")
        .and_then(serde_json::Value::as_str)
        .context("message content missing data")?;
    Ok(schedules_v1::Message {
        content: MessageField::some(content_v1alpha1::Content {
            content_type: content
                .get("content_type")
                .or_else(|| content.get("contentType"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("application/json")
                .to_string(),
            data: data.as_bytes().to_vec(),
        }),
        headers: Vec::new(),
    })
}

fn required_string(value: &serde_json::Value, field: &str) -> Result<String> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("missing {field}"))
}

#[cfg(test)]
mod tests;
