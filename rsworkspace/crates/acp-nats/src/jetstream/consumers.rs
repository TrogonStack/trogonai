use async_nats::jetstream::consumer::pull::Config;
use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, ReplayPolicy};

pub fn prompt_notifications_consumer(prefix: &str, session_id: &str, req_id: &str) -> Config {
    Config {
        filter_subject: format!("{prefix}.session.{session_id}.agent.update.{req_id}"),
        deliver_policy: DeliverPolicy::All,
        ack_policy: AckPolicy::Explicit,
        replay_policy: ReplayPolicy::Instant,
        ..Default::default()
    }
}

pub fn prompt_response_consumer(prefix: &str, session_id: &str, req_id: &str) -> Config {
    Config {
        filter_subject: format!("{prefix}.session.{session_id}.agent.prompt.response.{req_id}"),
        deliver_policy: DeliverPolicy::All,
        ack_policy: AckPolicy::Explicit,
        replay_policy: ReplayPolicy::Instant,
        ..Default::default()
    }
}

/// Consumer for the COMMANDS stream that receives all commands.
///
/// No filter needed — the COMMANDS stream already contains only command subjects.
/// The stream-level subject list acts as the filter.
pub fn runner_commands_consumer() -> Config {
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

    #[test]
    fn prompt_notifications_consumer_filter() {
        let config = prompt_notifications_consumer("acp", "sess-1", "req-abc");
        assert_eq!(
            config.filter_subject,
            "acp.session.sess-1.agent.update.req-abc"
        );
    }

    #[test]
    fn prompt_notifications_consumer_delivers_all() {
        let config = prompt_notifications_consumer("acp", "s1", "r1");
        assert_eq!(config.deliver_policy, DeliverPolicy::All);
        assert_eq!(config.ack_policy, AckPolicy::Explicit);
        assert_eq!(config.replay_policy, ReplayPolicy::Instant);
    }

    #[test]
    fn prompt_response_consumer_filter() {
        let config = prompt_response_consumer("acp", "sess-1", "req-abc");
        assert_eq!(
            config.filter_subject,
            "acp.session.sess-1.agent.prompt.response.req-abc"
        );
    }

    #[test]
    fn runner_commands_consumer_delivers_all() {
        let config = runner_commands_consumer();
        assert_eq!(config.deliver_policy, DeliverPolicy::All);
        assert_eq!(config.ack_policy, AckPolicy::Explicit);
    }

    #[test]
    fn runner_commands_consumer_no_filter() {
        let config = runner_commands_consumer();
        assert_eq!(config.filter_subject, String::new());
    }

    #[test]
    fn custom_prefix_in_consumers() {
        let config = prompt_response_consumer("myapp", "s1", "r1");
        assert_eq!(
            config.filter_subject,
            "myapp.session.s1.agent.prompt.response.r1"
        );
    }
}
