use std::fmt::Debug;

use buffa::json_helpers::ProtoElemJson;
use buffa::{Enumeration, HasMessageView, MessageView};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use super::assert_proto_sequence;

use crate::scheduler::schedules::checkpoints_v1::{ReconcileOutcome as Outcome, ScheduleCheckpointStatus as Status};
use crate::scheduler::schedules::state_v1::StateValue as State;
use crate::scheduler::schedules::{checkpoints_v1, projections_v1, v1};

macro_rules! assert_presence {
    ($message:ty, $present_wire:expr, $($has:ident),+ $(,)?) => {{
        let absent = <$message>::decode_view(b"").expect("absent required fields");
        let present = <$message>::decode_view($present_wire).expect("explicit default required fields");
        $(
            assert!(!absent.$has(), "absent field {}", stringify!($has));
            assert!(present.$has(), "present field {}", stringify!($has));
        )+
        assert_eq!(absent.to_owned_message().expect("owned absent"), <$message>::default());
    }};
}

macro_rules! schema_presence_contracts {
    ($name:ident, $schema:ident) => {
        mod $name {
            use super::*;
            use $schema as schema;

            #[test]
            fn explicit_default_schedule_fields_retain_required_presence() {
                assert_presence!(schema::schedule::At, b"\x0a\x00", has_at);
                assert_presence!(schema::schedule::Every, b"\x0a\x00", has_every);
                assert_presence!(
                    schema::schedule::Cron,
                    b"\x0a\x00\x12\x00",
                    has_expr,
                    has_timezone
                );
                assert_presence!(
                    schema::schedule::RRule,
                    b"\x0a\x00\x12\x00\x1a\x00",
                    has_dtstart,
                    has_rrule,
                    has_timezone
                );
            }

            #[test]
            fn explicit_empty_delivery_fields_retain_required_presence() {
                assert_presence!(schema::delivery::NatsMessage, b"\x0a\x00", has_subject);
                assert_presence!(
                    schema::delivery::nats_message::LatestFromSubject,
                    b"\x0a\x00",
                    has_subject
                );
                assert_presence!(schema::Message, b"\x0a\x00", has_content);
                assert_presence!(schema::Header, b"\x0a\x00\x12\x00", has_name, has_value);
            }
        }
    };
}

schema_presence_contracts!(live_tests, v1);
schema_presence_contracts!(checkpoint_tests, checkpoints_v1);
schema_presence_contracts!(projection_tests, projections_v1);

#[test]
fn lifecycle_message_presence_is_independent_of_empty_values() {
    assert_presence!(
        v1::CreateSchedule,
        b"\x0a\x00\x12\x00\x1a\x00\x22\x00\x2a\x00",
        has_schedule_id,
        has_status,
        has_schedule,
        has_delivery,
        has_message
    );
    assert_presence!(
        v1::ScheduleCreated,
        b"\x0a\x00\x12\x00\x1a\x00\x22\x00\x2a\x00",
        has_schedule_id,
        has_status,
        has_schedule,
        has_delivery,
        has_message
    );
    assert_presence!(
        projections_v1::ScheduleProjection,
        b"\x0a\x00\x12\x00\x32\x00\x3a\x00\x42\x00",
        has_schedule_id,
        has_status,
        has_schedule,
        has_delivery,
        has_message
    );
    assert_presence!(
        v1::ScheduleOccurrenceScheduled,
        b"\x0a\x00\x1a\x00\x22\x00",
        has_schedule_id,
        has_occurrence_at,
        has_scheduled_at
    );
    assert_presence!(
        v1::ScheduleOccurrenceRecorded,
        b"\x0a\x00\x1a\x00\x22\x00",
        has_schedule_id,
        has_occurrence_at,
        has_recorded_at
    );
    assert_presence!(v1::ScheduleCompleted, b"\x0a\x00", has_schedule_id);
    assert_presence!(v1::PauseSchedule, b"\x0a\x00", has_schedule_id);
    assert_presence!(v1::ResumeSchedule, b"\x0a\x00", has_schedule_id);
    assert_presence!(v1::RemoveSchedule, b"\x0a\x00", has_schedule_id);
    assert_presence!(v1::SchedulePaused, b"\x0a\x00", has_schedule_id);
    assert_presence!(v1::ScheduleResumed, b"\x0a\x00", has_schedule_id);
    assert_presence!(v1::ScheduleRemoved, b"\x0a\x00", has_schedule_id);
}

fn assert_enum_contract<E>(variants: &[(E, i32, &str)])
where
    E: Enumeration + Serialize + DeserializeOwned + PartialEq + Debug + Copy + Default + ProtoElemJson + 'static,
{
    let expected_values: Vec<E> = variants.iter().map(|&(value, _, _)| value).collect();
    assert_eq!(E::values(), expected_values);
    assert_proto_sequence(
        expected_values,
        json!(variants.iter().map(|&(_, _, name)| name).collect::<Vec<_>>()),
    );
    for &(variant, number, name) in variants {
        assert_eq!(variant.to_i32(), number);
        assert_eq!(variant.proto_name(), name);
        assert_eq!(E::from_i32(number), Some(variant));
        assert_eq!(E::from_proto_name(name), Some(variant));
        assert_eq!(serde_json::to_value(variant).expect("enum JSON"), json!(name));
        assert_eq!(serde_json::from_value::<E>(json!(name)).expect("named enum"), variant);
        assert_eq!(
            serde_json::from_value::<E>(json!(number)).expect("numeric enum"),
            variant
        );
        let signed = serde::de::value::I64Deserializer::<serde::de::value::Error>::new(i64::from(number));
        assert_eq!(E::deserialize(signed).expect("signed numeric enum"), variant);
    }
    assert_eq!(
        serde_json::from_value::<E>(json!(null)).expect("null enum default"),
        E::default()
    );
    for value in [
        json!(-1),
        json!(100),
        json!(i64::MIN),
        json!(u64::MAX),
        json!("UNKNOWN_VALUE"),
        json!(true),
        json!([]),
    ] {
        assert!(
            serde_json::from_value::<E>(value.clone()).is_err(),
            "invalid closed enum {value}"
        );
    }
    assert_eq!(E::from_i32(-1), None);
    assert_eq!(E::from_proto_name("UNKNOWN_VALUE"), None);
}

#[test]
fn checkpoint_enum_numbers_and_names_are_stable() {
    assert_enum_contract(&[
        (Status::Unspecified, 0, "SCHEDULE_CHECKPOINT_STATUS_UNSPECIFIED"),
        (Status::Scheduled, 1, "SCHEDULE_CHECKPOINT_STATUS_SCHEDULED"),
        (Status::Paused, 2, "SCHEDULE_CHECKPOINT_STATUS_PAUSED"),
        (Status::Removed, 3, "SCHEDULE_CHECKPOINT_STATUS_REMOVED"),
        (Status::Unsupported, 4, "SCHEDULE_CHECKPOINT_STATUS_UNSUPPORTED"),
        (Status::Expired, 5, "SCHEDULE_CHECKPOINT_STATUS_EXPIRED"),
    ]);
    assert_enum_contract(&[
        (Outcome::Unspecified, 0, "RECONCILE_OUTCOME_UNSPECIFIED"),
        (Outcome::Published, 1, "RECONCILE_OUTCOME_PUBLISHED"),
        (Outcome::Purged, 2, "RECONCILE_OUTCOME_PURGED"),
        (Outcome::StoredPaused, 3, "RECONCILE_OUTCOME_STORED_PAUSED"),
        (Outcome::Unsupported, 4, "RECONCILE_OUTCOME_UNSUPPORTED"),
        (Outcome::Expired, 5, "RECONCILE_OUTCOME_EXPIRED"),
        (Outcome::DuplicateStale, 6, "RECONCILE_OUTCOME_DUPLICATE_STALE"),
    ]);
}

#[test]
fn state_enum_numbers_and_names_are_stable() {
    assert_enum_contract(&[
        (State::Unspecified, 0, "STATE_VALUE_UNSPECIFIED"),
        (State::Missing, 1, "STATE_VALUE_MISSING"),
        (State::PresentEnabled, 2, "STATE_VALUE_PRESENT_ENABLED"),
        (State::PresentDisabled, 3, "STATE_VALUE_PRESENT_DISABLED"),
        (State::Deleted, 4, "STATE_VALUE_DELETED"),
    ]);
}
