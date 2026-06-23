    use super::*;

    fn p(s: &str) -> A2aPrefix {
        A2aPrefix::new(s.to_string()).expect("test prefix")
    }

    fn tid(s: &str) -> A2aTaskId {
        A2aTaskId::new(s).expect("test task id")
    }

    fn rid(s: &str) -> ReqId {
        ReqId::from_test(s)
    }

    #[test]
    fn gateway_events_consumer_uses_durable_full_task_filter() {
        let config = gateway_events_consumer(&p("a2a"), "A2A_GATEWAY_EVENTS", 1024);
        assert_eq!(config.durable_name.as_deref(), Some("A2A_GATEWAY_EVENTS"));
        assert_eq!(config.filter_subject, "a2a.tasks.*.events.*");
        assert_eq!(config.max_ack_pending, 1024);
        assert_eq!(config.ack_policy, AckPolicy::Explicit);
    }

    #[test]
    fn stream_events_consumer_filter_uses_wildcard_task_id() {
        let config = stream_events_consumer(&p("a2a"), &rid("r1"));
        assert_eq!(config.filter_subject, "a2a.tasks.*.events.r1");
    }

    #[test]
    fn stream_events_consumer_delivers_all() {
        let config = stream_events_consumer(&p("a2a"), &rid("r1"));
        assert_eq!(config.deliver_policy, DeliverPolicy::All);
        assert_eq!(config.ack_policy, AckPolicy::Explicit);
        assert_eq!(config.replay_policy, ReplayPolicy::Instant);
    }

    #[test]
    fn resubscribe_consumer_filter_matches_all_req_ids() {
        let config = resubscribe_consumer(&p("a2a"), &tid("t1"), 100);
        assert_eq!(config.filter_subject, "a2a.tasks.t1.events.*");
    }

    #[test]
    fn resubscribe_consumer_starts_at_last_seq_plus_one() {
        let config = resubscribe_consumer(&p("a2a"), &tid("t1"), 42);
        assert_eq!(
            config.deliver_policy,
            DeliverPolicy::ByStartSequence { start_sequence: 43 }
        );
    }

    #[test]
    fn resubscribe_consumer_handles_zero_seq() {
        let config = resubscribe_consumer(&p("a2a"), &tid("t1"), 0);
        assert_eq!(
            config.deliver_policy,
            DeliverPolicy::ByStartSequence { start_sequence: 1 }
        );
    }

    #[test]
    fn resubscribe_consumer_saturates_at_u64_max() {
        let config = resubscribe_consumer(&p("a2a"), &tid("t1"), u64::MAX);
        assert_eq!(
            config.deliver_policy,
            DeliverPolicy::ByStartSequence {
                start_sequence: u64::MAX
            }
        );
    }

    #[test]
    fn consumers_use_explicit_ack_and_instant_replay() {
        let stream = stream_events_consumer(&p("a2a"), &rid("r1"));
        let resub = resubscribe_consumer(&p("a2a"), &tid("t1"), 1);
        assert_eq!(stream.ack_policy, AckPolicy::Explicit);
        assert_eq!(stream.replay_policy, ReplayPolicy::Instant);
        assert_eq!(resub.ack_policy, AckPolicy::Explicit);
        assert_eq!(resub.replay_policy, ReplayPolicy::Instant);
    }

    #[test]
    fn custom_prefix_in_consumers() {
        let stream = stream_events_consumer(&p("myapp"), &rid("r1"));
        assert_eq!(stream.filter_subject, "myapp.tasks.*.events.r1");
        let resub = resubscribe_consumer(&p("myapp"), &tid("t1"), 5);
        assert_eq!(resub.filter_subject, "myapp.tasks.t1.events.*");
    }

    #[test]
    fn consumers_set_five_minute_inactive_threshold() {
        let stream = stream_events_consumer(&p("a2a"), &rid("r1"));
        let resub = resubscribe_consumer(&p("a2a"), &tid("t1"), 0);
        assert_eq!(stream.inactive_threshold, Duration::from_secs(300));
        assert_eq!(resub.inactive_threshold, Duration::from_secs(300));
    }
