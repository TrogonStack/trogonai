    use super::*;

    #[test]
    fn default_a2a_prefix_constructs() {
        assert_eq!(default_a2a_prefix().as_str(), "a2a");
    }

    #[test]
    fn extract_last_sequence_reads_canonical_and_legacy_keys() {
        let last_seq = serde_json::json!({"lastSeq": 7});
        assert_eq!(extract_last_sequence(&last_seq), Some(7));

        let metadata_last_event_id = serde_json::json!({"metadata": {"lastEventId": "42"}});
        assert_eq!(extract_last_sequence(&metadata_last_event_id), Some(42));

        let resume_string = serde_json::json!({"resume_from_sequence": "13"});
        assert_eq!(extract_last_sequence(&resume_string), Some(13));

        let missing = serde_json::json!({"unrelated": 99});
        assert_eq!(extract_last_sequence(&missing), None);
    }
