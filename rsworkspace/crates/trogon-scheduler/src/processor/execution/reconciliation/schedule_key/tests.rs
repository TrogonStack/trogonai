use super::*;

    fn schedule_id(raw: &str) -> ScheduleId {
        ScheduleId::parse(raw).unwrap()
    }

    #[test]
    fn derivation_is_deterministic_for_the_same_schedule_id() {
        let first = ScheduleKey::derive(&schedule_id("orders/created"));
        let second = ScheduleKey::derive(&schedule_id("orders/created"));

        assert_eq!(first, second);
        assert_eq!(first.as_uuid(), second.as_uuid());
    }

    #[test]
    fn different_schedule_ids_derive_different_keys() {
        let first = ScheduleKey::derive(&schedule_id("orders/created"));
        let second = ScheduleKey::derive(&schedule_id("orders/updated"));

        assert_ne!(first, second);
    }

    #[test]
    fn simple_representation_is_thirty_two_lowercase_hex_digits() {
        let key = ScheduleKey::derive(&schedule_id("日次バックアップ"));
        let simple = key.simple();

        assert_eq!(simple.len(), 32);
        assert!(
            simple
                .chars()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
        );
    }

    #[test]
    fn non_nats_token_schedule_ids_still_derive_a_token_safe_key() {
        for raw in ["report.v2", "orders/created", "ns:thing", "user@host", "café-nightly"] {
            let simple = ScheduleKey::derive(&schedule_id(raw)).simple();
            assert_eq!(simple.len(), 32, "{raw}");
        }
    }

    #[test]
    fn stream_routing_id_preserves_key_derivation_for_arbitrary_stream_ids() {
        let raw = "orders/created";
        let routing_id = StreamRoutingId::from(raw);

        assert_eq!(routing_id.as_str(), raw);
        assert_eq!(
            ScheduleKey::for_stream(&routing_id),
            ScheduleKey::derive(&schedule_id(raw))
        );
    }

    #[test]
    fn stream_routing_id_accepts_owned_strings() {
        let routing_id = StreamRoutingId::from("orders/created".to_string());
        assert_eq!(routing_id.as_str(), "orders/created");
    }
