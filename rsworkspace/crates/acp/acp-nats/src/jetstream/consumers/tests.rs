use super::*;

fn p(s: &str) -> AcpPrefix {
    AcpPrefix::new(s).expect("test prefix")
}

fn sid(s: &str) -> AcpSessionId {
    AcpSessionId::new(s).expect("test session id")
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
    let config = response_consumer(&p("acp"), &sid("sess-1"));
    assert_eq!(config.filter_subject, "acp.v1.session.sess-1.agent.response");
}

#[test]
fn response_consumer_delivers_new() {
    let config = response_consumer(&p("acp"), &sid("s1"));
    assert_eq!(config.deliver_policy, DeliverPolicy::New);
    assert_eq!(config.ack_policy, AckPolicy::Explicit);
    assert_eq!(config.replay_policy, ReplayPolicy::Instant);
}

#[test]
fn response_consumer_custom_prefix() {
    let config = response_consumer(&p("myapp"), &sid("s1"));
    assert_eq!(config.filter_subject, "myapp.v1.session.s1.agent.response");
}

#[test]
fn consumer_filters_carry_no_request_id() {
    let filter = response_consumer(&p("acp"), &sid("sess-1")).filter_subject;
    assert!(
        !filter
            .split('.')
            .any(trogon_nats::subject_conformance::looks_like_request_id),
        "{filter}"
    );
}
