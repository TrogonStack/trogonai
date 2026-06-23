use super::*;

    #[test]
    fn event_data_new_borrows_event_type_and_payload() {
        let payload = [1, 2, 3];
        let event = EventData::new("test.event", &payload);

        assert_eq!(event.event_type, "test.event");
        assert_eq!(event.payload, payload);
    }

    #[test]
    fn decoded_outcome_exposes_event() {
        let outcome = EventDecodeOutcome::Decoded("event");

        assert_eq!(outcome.as_decoded(), Some(&"event"));
        assert_eq!(outcome.into_decoded(), Some("event"));
    }

    #[test]
    fn skipped_outcome_has_no_event() {
        let outcome = EventDecodeOutcome::<&str>::Skipped;

        assert_eq!(outcome.as_decoded(), None);
        assert_eq!(outcome.into_decoded(), None);
    }
