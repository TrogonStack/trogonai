use std::time::Duration;

use async_nats::HeaderMap;
use bytes::Bytes;
use serde::Deserialize;
use serde_json::value::RawValue;
use tracing::{debug, info, warn};
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{ClaimCheckPublisher, JetStreamContext, JetStreamPublisher, ObjectStorePut};

use super::config::DiscordConfig;
use super::constants::{NATS_HEADER_EVENT_NAME, NATS_HEADER_GUILD_ID};

const GATEWAY_OP_DISPATCH: u8 = 0;

pub async fn provision<C: JetStreamContext>(js: &C, config: &DiscordConfig) -> Result<(), C::Error> {
    js.get_or_create_stream(async_nats::jetstream::stream::Config {
        name: config.stream_name.to_string(),
        subjects: vec![format!("{}.>", config.subject_prefix)],
        max_age: config.stream_max_age.into(),
        ..Default::default()
    })
    .await?;

    let max_age_secs = Duration::from(config.stream_max_age).as_secs();
    info!(
        stream = %config.stream_name,
        max_age_secs,
        "JetStream stream ready"
    );
    Ok(())
}

#[derive(Deserialize)]
struct GatewayPayload<'a> {
    op: u8,
    #[serde(default)]
    t: Option<&'a str>,
    #[serde(default, borrow)]
    d: Option<&'a RawValue>,
}

pub struct GatewayBridge<P: JetStreamPublisher, S: ObjectStorePut> {
    publisher: ClaimCheckPublisher<P, S>,
    subject_prefix: NatsToken,
    nats_ack_timeout: Duration,
}

impl<P: JetStreamPublisher, S: ObjectStorePut> GatewayBridge<P, S> {
    pub fn new(publisher: ClaimCheckPublisher<P, S>, subject_prefix: NatsToken, nats_ack_timeout: Duration) -> Self {
        Self {
            publisher,
            subject_prefix,
            nats_ack_timeout,
        }
    }

    pub async fn dispatch(&self, raw: &str) {
        let payload: GatewayPayload = match serde_json::from_str(raw) {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, "failed to parse gateway payload");
                return;
            }
        };

        if payload.op != GATEWAY_OP_DISPATCH {
            return;
        }

        let Some(event_type) = payload.t else {
            return;
        };

        let Some(data) = payload.d else {
            return;
        };

        let event_name = event_type.to_ascii_lowercase();
        let data_bytes = data.get().as_bytes();

        let guild_id = extract_guild_id(data_bytes);
        let dedup_id = extract_dedup_id(&event_name, data_bytes);

        self.publish_bytes(&event_name, guild_id, dedup_id, Bytes::copy_from_slice(data_bytes))
            .await;
    }

    async fn publish_bytes(&self, event_name: &str, guild_id: Option<u64>, msg_id: Option<String>, payload: Bytes) {
        let subject = format!("{}.{}", self.subject_prefix, event_name);

        let mut headers = HeaderMap::new();
        headers.insert(NATS_HEADER_EVENT_NAME, event_name);
        if let Some(gid) = guild_id {
            headers.insert(NATS_HEADER_GUILD_ID, gid.to_string().as_str());
        }
        if let Some(ref id) = msg_id {
            headers.insert(async_nats::header::NATS_MESSAGE_ID, id.as_str());
        }

        let outcome = self
            .publisher
            .publish_event(subject, headers, payload, self.nats_ack_timeout)
            .await;

        if outcome.is_ok() {
            debug!(event = event_name, "published gateway event to NATS");
        } else {
            outcome.log_on_error(event_name);
        }
    }

    #[cfg(test)]
    async fn publish(
        &self,
        event_name: &str,
        guild_id: Option<u64>,
        msg_id: Option<String>,
        payload: &impl serde::Serialize,
    ) {
        let json = serde_json::to_vec(payload).expect("test payload must serialize");
        self.publish_bytes(event_name, guild_id, msg_id, Bytes::from(json))
            .await;
    }
}

fn extract_guild_id(data: &[u8]) -> Option<u64> {
    #[derive(Deserialize)]
    struct Probe {
        guild_id: Option<serde_json::Value>,
    }

    let probe: Probe = serde_json::from_slice(data).ok()?;
    match probe.guild_id? {
        serde_json::Value::String(s) => s.parse().ok(),
        serde_json::Value::Number(n) => n.as_u64(),
        _ => None,
    }
}

fn extract_dedup_id(event_name: &str, data: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(data).ok()?;

    match event_name {
        "message_create"
        | "message_delete"
        | "channel_create"
        | "channel_delete"
        | "guild_delete"
        | "interaction_create"
        | "thread_create"
        | "thread_delete"
        | "stage_instance_create"
        | "stage_instance_delete"
        | "guild_scheduled_event_create"
        | "guild_scheduled_event_delete"
        | "guild_audit_log_entry_create"
        | "integration_create"
        | "integration_delete"
        | "entitlement_create"
        | "entitlement_delete"
        | "auto_moderation_rule_create"
        | "auto_moderation_rule_delete" => {
            let id = v.get("id")?.as_str()?;
            Some(format!("{event_name}:{id}"))
        }
        "message_delete_bulk" => {
            let channel_id = v.get("channel_id")?.as_str()?;
            let ids: Vec<&str> = v.get("ids")?.as_array()?.iter().filter_map(|v| v.as_str()).collect();
            Some(format!("{event_name}:{channel_id}:{}", ids.join(",")))
        }
        "guild_role_create" => {
            let id = v.get("role")?.get("id")?.as_str()?;
            Some(format!("{event_name}:{id}"))
        }
        "guild_role_delete" => {
            let id = v.get("role_id")?.as_str()?;
            Some(format!("{event_name}:{id}"))
        }
        "invite_create" | "invite_delete" => {
            let code = v.get("code")?.as_str()?;
            Some(format!("{event_name}:{code}"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
