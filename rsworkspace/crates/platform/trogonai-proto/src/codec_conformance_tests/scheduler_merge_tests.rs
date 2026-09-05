use buffa::{DecodeError, Message};
use serde_json::json;

use super::{assert_collection_limit, assert_json_codec, assert_malformed, assert_wire_codec};
use crate::scheduler::schedules::{checkpoints_v1, projections_v1, v1};

macro_rules! schema_merge_contracts {
    ($name:ident, $schema:ident) => {
        mod $name {
            use super::*;
            use $schema as schema;

            #[test]
            fn repeated_cron_timezone_fragments_preserve_zone_and_version() {
                let expected = assert_json_codec::<schema::Schedule>(json!({"cron": {
                    "expr": "0 9 * * *", "timezone": {"id": "UTC", "version": "2025b"}
                }}));
                let wire = b"\x1a\x12\x0a\x090 9 * * *\x12\x05\x0a\x03UTC\x1a\x09\x12\x07\x12\x052025b";
                assert_wire_codec(wire, &expected);
                assert_malformed::<schema::Schedule>(
                    &[wire.as_slice(), b"\x1a\x05\x12\x03\x12\x01\xff"].concat(),
                    DecodeError::InvalidUtf8,
                );
            }

            #[test]
            fn scheduler_collections_charge_empty_elements_against_memory_budget() {
                assert_collection_limit::<schema::Message>(b"\x12\x00");
                assert_collection_limit::<schema::schedule::RRule>(b"\x22\x00");
                assert_collection_limit::<schema::schedule::RRule>(b"\x2a\x00");
            }

            #[test]
            fn payload_conversion_selects_the_corresponding_oneof_variant() {
                let at: schema::schedule::At = serde_json::from_value(json!({"at": "2026-01-01T00:00:00Z"})).expect("at");
                let every: schema::schedule::Every = serde_json::from_value(json!({"every": "60s"})).expect("every");
                let cron: schema::schedule::Cron = serde_json::from_value(json!({"expr": "0 9 * * *", "timezone": {"id": "UTC"}})).expect("cron");
                let rrule: schema::schedule::RRule = serde_json::from_value(json!({"rrule": "FREQ=DAILY", "dtstart": "2026-01-01T00:00:00Z", "timezone": {"id": "UTC"}})).expect("rrule");
                for (kind, expected) in [
                    (Option::<schema::schedule::Kind>::from(at), json!({"at": {"at": "2026-01-01T00:00:00Z"}})),
                    (Option::<schema::schedule::Kind>::from(every), json!({"every": {"every": "60s"}})),
                    (Option::<schema::schedule::Kind>::from(cron), json!({"cron": {"expr": "0 9 * * *", "timezone": {"id": "UTC"}}})),
                    (Option::<schema::schedule::Kind>::from(rrule), json!({"rrule": {"rrule": "FREQ=DAILY", "dtstart": "2026-01-01T00:00:00Z", "timezone": {"id": "UTC"}}})),
                ] {
                    let message = schema::Schedule { kind };
                    let encoded = serde_json::to_value(&message).expect("promoted schedule JSON");
                    assert_eq!(encoded, expected);
                    assert_wire_codec(&message.encode_to_vec(), &message);
                }
                let payload = schema::delivery::nats_message::LatestFromSubject { subject: "jobs.template".to_owned() };
                let source = schema::delivery::nats_message::Source { kind: payload.into() };
                assert_eq!(serde_json::to_value(&source).expect("promoted source"), json!({"latestFromSubject": {"subject": "jobs.template"}}));
                let delivery = schema::Delivery { kind: schema::delivery::NatsMessage {
                    subject: "jobs.backup".to_owned(), source: buffa::MessageField::some(source), ..Default::default()
                }.into() };
                assert_eq!(serde_json::to_value(delivery).expect("promoted delivery"), json!({"natsMessage": {
                    "subject": "jobs.backup", "source": {"latestFromSubject": {"subject": "jobs.template"}}
                }}));
            }
        }
    };
}

schema_merge_contracts!(live_tests, v1);
schema_merge_contracts!(checkpoint_tests, checkpoints_v1);
schema_merge_contracts!(projection_tests, projections_v1);
