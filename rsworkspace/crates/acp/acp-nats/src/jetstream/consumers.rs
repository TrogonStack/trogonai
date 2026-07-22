use async_nats::jetstream::consumer::pull::Config;
use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, ReplayPolicy};

use crate::acp_prefix::AcpPrefix;
use crate::req_id::ReqId;
use crate::session_id::AcpSessionId;

pub fn prompt_notifications_consumer(prefix: &AcpPrefix, session_id: &AcpSessionId, req_id: &ReqId) -> Config {
    let pfx = prefix.as_str();
    let sid = session_id.as_str();
    Config {
        filter_subject: format!("{pfx}.session.{sid}.agent.update.{req_id}"),
        deliver_policy: DeliverPolicy::All,
        ack_policy: AckPolicy::Explicit,
        replay_policy: ReplayPolicy::Instant,
        ..Default::default()
    }
}

pub fn prompt_response_consumer(prefix: &AcpPrefix, session_id: &AcpSessionId, req_id: &ReqId) -> Config {
    let pfx = prefix.as_str();
    let sid = session_id.as_str();
    Config {
        filter_subject: format!("{pfx}.session.{sid}.agent.prompt.response.{req_id}"),
        deliver_policy: DeliverPolicy::All,
        ack_policy: AckPolicy::Explicit,
        replay_policy: ReplayPolicy::Instant,
        ..Default::default()
    }
}

pub fn response_consumer(prefix: &AcpPrefix, session_id: &AcpSessionId, req_id: &ReqId) -> Config {
    let pfx = prefix.as_str();
    let sid = session_id.as_str();
    Config {
        filter_subject: format!("{pfx}.session.{sid}.agent.response.{req_id}"),
        deliver_policy: DeliverPolicy::All,
        ack_policy: AckPolicy::Explicit,
        replay_policy: ReplayPolicy::Instant,
        ..Default::default()
    }
}

/// Observer consumer for the COMMANDS stream.
///
/// Acks messages for audit persistence. No filter needed — the stream-level
/// subject list already scopes to session-scoped commands only.
pub fn commands_observer() -> Config {
    Config {
        deliver_policy: DeliverPolicy::All,
        ack_policy: AckPolicy::Explicit,
        replay_policy: ReplayPolicy::Instant,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests;
