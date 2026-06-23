use super::*;

    #[test]
    fn failure_record_round_trips_and_keys_deterministically() {
        let record = ProcessingFailureRecord::new(
            "SCHEDULER_SCHEDULE_EVENTS",
            StreamPosition::try_new(42).unwrap(),
            Some("event-42".to_string()),
            "payload could not be decoded",
            DateTime::parse_from_rfc3339("2026-06-04T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );

        assert_eq!(record.key(), "failure.v1.SCHEDULER_SCHEDULE_EVENTS.42");
        let decoded = decode_failure_record(&encode_failure_record(&record).unwrap()).unwrap();
        assert_eq!(decoded, record);
    }
