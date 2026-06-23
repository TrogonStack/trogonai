use super::*;

    fn p(s: &str) -> AcpPrefix {
        AcpPrefix::new(s).expect("test prefix")
    }

    fn sid(s: &str) -> AcpSessionId {
        AcpSessionId::new(s).expect("test session id")
    }

    fn rid(s: &str) -> ReqId {
        ReqId::from_test(s)
    }

    #[test]
    fn prompt_notifications_consumer_filter() {
        let config = prompt_notifications_consumer(&p("acp"), &sid("sess-1"), &rid("req-abc"));
        assert_eq!(config.filter_subject, "acp.session.sess-1.agent.update.req-abc");
    }

    #[test]
    fn prompt_notifications_consumer_delivers_all() {
        let config = prompt_notifications_consumer(&p("acp"), &sid("s1"), &rid("r1"));
        assert_eq!(config.deliver_policy, DeliverPolicy::All);
        assert_eq!(config.ack_policy, AckPolicy::Explicit);
        assert_eq!(config.replay_policy, ReplayPolicy::Instant);
    }

    #[test]
    fn prompt_response_consumer_filter() {
        let config = prompt_response_consumer(&p("acp"), &sid("sess-1"), &rid("req-abc"));
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
        let config = response_consumer(&p("acp"), &sid("sess-1"), &rid("req-abc"));
        assert_eq!(config.filter_subject, "acp.session.sess-1.agent.response.req-abc");
    }

    #[test]
    fn response_consumer_delivers_all() {
        let config = response_consumer(&p("acp"), &sid("s1"), &rid("r1"));
        assert_eq!(config.deliver_policy, DeliverPolicy::All);
        assert_eq!(config.ack_policy, AckPolicy::Explicit);
        assert_eq!(config.replay_policy, ReplayPolicy::Instant);
    }

    #[test]
    fn response_consumer_custom_prefix() {
        let config = response_consumer(&p("myapp"), &sid("s1"), &rid("r1"));
        assert_eq!(config.filter_subject, "myapp.session.s1.agent.response.r1");
    }

    #[test]
    fn custom_prefix_in_consumers() {
        let config = prompt_response_consumer(&p("myapp"), &sid("s1"), &rid("r1"));
        assert_eq!(config.filter_subject, "myapp.session.s1.agent.prompt.response.r1");
    }
