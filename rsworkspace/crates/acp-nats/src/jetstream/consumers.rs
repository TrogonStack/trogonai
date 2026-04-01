use async_nats::jetstream::consumer::pull::Config;
use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, ReplayPolicy};

use crate::acp_prefix::AcpPrefix;

pub fn prompt_notifications_consumer(prefix: &AcpPrefix, session_id: &str, req_id: &str) -> Config {
    let pfx = prefix.as_str();
    Config {
        filter_subject: format!("{pfx}.session.{session_id}.agent.update.{req_id}"),
        deliver_policy: DeliverPolicy::All,
        ack_policy: AckPolicy::Explicit,
        replay_policy: ReplayPolicy::Instant,
        ..Default::default()
    }
}

pub fn prompt_response_consumer(prefix: &AcpPrefix, session_id: &str, req_id: &str) -> Config {
    let pfx = prefix.as_str();
    Config {
        filter_subject: format!("{pfx}.session.{session_id}.agent.prompt.response.{req_id}"),
        deliver_policy: DeliverPolicy::All,
        ack_policy: AckPolicy::Explicit,
        replay_policy: ReplayPolicy::Instant,
        ..Default::default()
    }
}

pub fn response_consumer(prefix: &AcpPrefix, session_id: &str, req_id: &str) -> Config {
    let pfx = prefix.as_str();
    Config {
        filter_subject: format!("{pfx}.session.{session_id}.agent.response.{req_id}"),
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
mod tests {
    use super::*;

    fn p(s: &str) -> AcpPrefix {
        AcpPrefix::new(s).expect("test prefix")
    }

    #[test]
    fn prompt_notifications_consumer_filter() {
        let config = prompt_notifications_consumer(&p("acp"), "sess-1", "req-abc");
        assert_eq!(
            config.filter_subject,
            "acp.session.sess-1.agent.update.req-abc"
        );
    }

    #[test]
    fn prompt_notifications_consumer_delivers_all() {
        let config = prompt_notifications_consumer(&p("acp"), "s1", "r1");
        assert_eq!(config.deliver_policy, DeliverPolicy::All);
        assert_eq!(config.ack_policy, AckPolicy::Explicit);
        assert_eq!(config.replay_policy, ReplayPolicy::Instant);
    }

    #[test]
    fn prompt_response_consumer_filter() {
        let config = prompt_response_consumer(&p("acp"), "sess-1", "req-abc");
        assert_eq!(
            config.filter_subject,
            "acp.session.sess-1.agent.prompt.response.req-abc"
        );
    }

    #[test]
    fn commands_observer_delivers_all() {
        let config = commands_observer();
        assert_eq!(config.deliver_policy, DeliverPolicy::All);
        assert_eq!(config.ack_policy, AckPolicy::Explicit);
    }

    #[test]
    fn commands_observer_no_filter() {
        let config = commands_observer();
        assert_eq!(config.filter_subject, String::new());
    }

    #[test]
    fn response_consumer_filter() {
        let config = response_consumer(&p("acp"), "sess-1", "req-abc");
        assert_eq!(
            config.filter_subject,
            "acp.session.sess-1.agent.response.req-abc"
        );
    }

    #[test]
    fn response_consumer_delivers_all() {
        let config = response_consumer(&p("acp"), "s1", "r1");
        assert_eq!(config.deliver_policy, DeliverPolicy::All);
        assert_eq!(config.ack_policy, AckPolicy::Explicit);
        assert_eq!(config.replay_policy, ReplayPolicy::Instant);
    }

    #[test]
    fn response_consumer_custom_prefix() {
        let config = response_consumer(&p("myapp"), "s1", "r1");
        assert_eq!(config.filter_subject, "myapp.session.s1.agent.response.r1");
    }

    #[test]
    fn custom_prefix_in_consumers() {
        let config = prompt_response_consumer(&p("myapp"), "s1", "r1");
        assert_eq!(
            config.filter_subject,
            "myapp.session.s1.agent.prompt.response.r1"
        );
    }
}
