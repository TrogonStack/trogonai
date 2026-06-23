    use super::*;

    #[test]
    fn default_is_at_least_once() {
        assert_eq!(DeliverySemantics::default(), DeliverySemantics::AtLeastOnce);
    }

    #[test]
    fn idempotency_key_required_only_for_exactly_once() {
        assert!(!DeliverySemantics::AtLeastOnce.idempotency_key_required());
        assert!(
            DeliverySemantics::ExactlyOnce {
                idempotency_key_header: None
            }
            .idempotency_key_required()
        );
    }

    #[test]
    fn webhook_max_publish_attempts_uses_constant() {
        let semantics = DeliverySemantics::AtLeastOnce;
        assert_eq!(semantics.webhook_max_publish_attempts(), HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS);
        assert_eq!(
            semantics.jetstream_max_publish_attempts(),
            HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS
        );
    }

    #[test]
    fn nats_core_publish_max_attempts_collapses_to_one_under_exactly_once() {
        assert_eq!(
            DeliverySemantics::AtLeastOnce.nats_core_publish_max_attempts(),
            HTTP_PUSH_WEBHOOK_MAX_ATTEMPTS
        );
        assert_eq!(
            DeliverySemantics::ExactlyOnce {
                idempotency_key_header: None
            }
            .nats_core_publish_max_attempts(),
            1
        );
    }

    #[test]
    fn webhook_idempotency_carrier_returns_default_when_unset_under_exactly_once() {
        let semantics = DeliverySemantics::ExactlyOnce {
            idempotency_key_header: None,
        };
        let carrier = semantics.webhook_idempotency_carrier().unwrap();
        assert_eq!(carrier.as_str(), "idempotency-key");
    }

    #[test]
    fn webhook_idempotency_carrier_uses_custom_header_when_set() {
        let header = IdempotencyKeyHeader::try_from("X-Push-Key").unwrap();
        let semantics = DeliverySemantics::ExactlyOnce {
            idempotency_key_header: Some(header),
        };
        let carrier = semantics.webhook_idempotency_carrier().unwrap();
        assert_eq!(carrier.as_str(), "x-push-key");
    }

    #[test]
    fn webhook_idempotency_carrier_is_none_for_at_least_once() {
        assert!(DeliverySemantics::AtLeastOnce.webhook_idempotency_carrier().is_none());
    }

    #[test]
    fn merged_uses_delivery_semantics_when_supplied() {
        let merged = merged_request_delivery_semantics(Some(&serde_json::json!("exactlyOnce")), Some(false)).unwrap();
        assert!(merged.idempotency_key_required());
    }

    #[test]
    fn merged_falls_back_to_exactly_once_delivery_bool() {
        let merged = merged_request_delivery_semantics(None, Some(true)).unwrap();
        assert!(merged.idempotency_key_required());
    }

    #[test]
    fn merged_defaults_to_at_least_once_when_neither_set() {
        assert_eq!(
            merged_request_delivery_semantics(None, None).unwrap(),
            DeliverySemantics::AtLeastOnce
        );
        assert_eq!(
            merged_request_delivery_semantics(None, Some(false)).unwrap(),
            DeliverySemantics::AtLeastOnce
        );
    }

    #[test]
    fn parse_value_string_atleastonce_is_case_insensitive() {
        for raw in ["atLeastOnce", "ATLEASTONCE", "atleastonce"] {
            let parsed = parse_delivery_semantics_value(&serde_json::json!(raw)).unwrap();
            assert_eq!(parsed, DeliverySemantics::AtLeastOnce);
        }
    }

    #[test]
    fn parse_value_string_exactlyonce_is_case_insensitive() {
        for raw in ["exactlyOnce", "EXACTLYONCE", "exactlyonce"] {
            let parsed = parse_delivery_semantics_value(&serde_json::json!(raw)).unwrap();
            assert!(parsed.idempotency_key_required());
        }
    }

    #[test]
    fn parse_value_rejects_unknown_string_or_primitive() {
        assert!(matches!(
            parse_delivery_semantics_value(&serde_json::json!("unknown")),
            Err(DeliverySemanticsParseError::UnknownShape)
        ));
        assert!(matches!(
            parse_delivery_semantics_value(&serde_json::json!(42)),
            Err(DeliverySemanticsParseError::UnknownShape)
        ));
    }

    #[test]
    fn parse_object_atleastonce_in_camel_and_snake() {
        let parsed = parse_delivery_semantics_value(&serde_json::json!({"atLeastOnce": {}})).unwrap();
        assert_eq!(parsed, DeliverySemantics::AtLeastOnce);
        let parsed = parse_delivery_semantics_value(&serde_json::json!({"at_least_once": null})).unwrap();
        assert_eq!(parsed, DeliverySemantics::AtLeastOnce);
    }

    #[test]
    fn parse_object_exactlyonce_with_null_or_bool_yields_default_header() {
        let parsed = parse_delivery_semantics_value(&serde_json::json!({"exactlyOnce": null})).unwrap();
        assert_eq!(
            parsed,
            DeliverySemantics::ExactlyOnce {
                idempotency_key_header: None
            }
        );
        let parsed = parse_delivery_semantics_value(&serde_json::json!({"exactlyOnce": true})).unwrap();
        assert_eq!(
            parsed,
            DeliverySemantics::ExactlyOnce {
                idempotency_key_header: None
            }
        );
    }

    #[test]
    fn parse_object_exactlyonce_inner_object_parses_header() {
        let parsed =
            parse_delivery_semantics_value(&serde_json::json!({"exactlyOnce": {"idempotencyKeyHeader": "X-Push-Key"}}))
                .unwrap();
        let expected = DeliverySemantics::ExactlyOnce {
            idempotency_key_header: Some(IdempotencyKeyHeader::try_from("X-Push-Key").unwrap()),
        };
        assert_eq!(parsed, expected);
    }

    #[test]
    fn parse_object_exactlyonce_rejects_bad_header_name() {
        let err = parse_delivery_semantics_value(
            &serde_json::json!({"exactlyOnce": {"idempotencyKeyHeader": "not a valid header"}}),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            DeliverySemanticsParseError::InvalidIdempotencyHeaderName(_)
        ));
    }

    #[test]
    fn parse_object_exactlyonce_rejects_non_object_non_null_non_bool() {
        let err = parse_delivery_semantics_value(&serde_json::json!({"exactlyOnce": 42})).unwrap_err();
        assert!(matches!(err, DeliverySemanticsParseError::UnknownShape));
    }

    #[test]
    fn parse_object_without_known_keys_is_unknown_shape() {
        let err = parse_delivery_semantics_value(&serde_json::json!({"foo": "bar"})).unwrap_err();
        assert!(matches!(err, DeliverySemanticsParseError::UnknownShape));
    }

    #[test]
    fn parse_object_with_both_at_least_once_and_exactly_once_is_conflicting() {
        let err =
            parse_delivery_semantics_value(&serde_json::json!({"atLeastOnce": {}, "exactlyOnce": null})).unwrap_err();
        assert!(matches!(err, DeliverySemanticsParseError::ConflictingShape));
    }

    #[test]
    fn parse_object_exactlyonce_false_degrades_to_at_least_once() {
        let parsed = parse_delivery_semantics_value(&serde_json::json!({"exactlyOnce": false})).unwrap();
        assert_eq!(parsed, DeliverySemantics::AtLeastOnce);
        let parsed = parse_delivery_semantics_value(&serde_json::json!({"exactly_once": false})).unwrap();
        assert_eq!(parsed, DeliverySemantics::AtLeastOnce);
    }

    #[test]
    fn parse_object_exactlyonce_inner_object_parses_snake_case_header() {
        let parsed = parse_delivery_semantics_value(
            &serde_json::json!({"exactlyOnce": {"idempotency_key_header": "X-Push-Key"}}),
        )
        .unwrap();
        let expected = DeliverySemantics::ExactlyOnce {
            idempotency_key_header: Some(IdempotencyKeyHeader::try_from("X-Push-Key").unwrap()),
        };
        assert_eq!(parsed, expected);
    }

    #[test]
    fn parse_object_with_snake_case_both_keys_is_conflicting() {
        let err = parse_delivery_semantics_value(&serde_json::json!({"at_least_once": null, "exactly_once": true}))
            .unwrap_err();
        assert!(matches!(err, DeliverySemanticsParseError::ConflictingShape));
    }

    #[test]
    fn upsert_at_least_once_removes_field() {
        let mut map = serde_json::Map::new();
        map.insert("deliverySemantics".into(), serde_json::json!({"existing": "x"}));
        upsert_delivery_semantics_on_push_config_json_object(&mut map, &DeliverySemantics::AtLeastOnce);
        assert!(!map.contains_key("deliverySemantics"));
    }

    #[test]
    fn upsert_exactly_once_writes_field() {
        let mut map = serde_json::Map::new();
        upsert_delivery_semantics_on_push_config_json_object(
            &mut map,
            &DeliverySemantics::ExactlyOnce {
                idempotency_key_header: None,
            },
        );
        assert!(map["deliverySemantics"]["exactlyOnce"].is_object());
    }

    #[test]
    fn upsert_exactly_once_writes_custom_header() {
        let mut map = serde_json::Map::new();
        let header = IdempotencyKeyHeader::try_from("X-Push-Key").unwrap();
        upsert_delivery_semantics_on_push_config_json_object(
            &mut map,
            &DeliverySemantics::ExactlyOnce {
                idempotency_key_header: Some(header),
            },
        );
        assert_eq!(
            map["deliverySemantics"]["exactlyOnce"]["idempotencyKeyHeader"]
                .as_str()
                .unwrap(),
            "x-push-key"
        );
    }

    #[test]
    fn parse_error_display_covers_every_variant() {
        use std::error::Error as _;
        let unknown = DeliverySemanticsParseError::UnknownShape;
        assert!(unknown.to_string().contains("atLeastOnce"));
        assert!(unknown.source().is_none());

        let conflicting = DeliverySemanticsParseError::ConflictingShape;
        assert!(conflicting.to_string().contains("both"));
        assert!(conflicting.source().is_none());

        let invalid = DeliverySemanticsParseError::InvalidIdempotencyHeaderName(
            IdempotencyKeyHeader::try_from("not a valid header").unwrap_err(),
        );
        assert!(invalid.source().is_some());
        assert!(!invalid.to_string().is_empty());
    }
