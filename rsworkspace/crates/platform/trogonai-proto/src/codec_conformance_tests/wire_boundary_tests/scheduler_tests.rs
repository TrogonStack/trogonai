use super::{assert_embedded_validation, assert_field_wire_types};
use crate::google::r#type::{DateTime, TimeZone};
use crate::scheduler::schedules::{checkpoints_v1, projections_v1, state_v1, v1};

#[test]
fn datetime_fields_validate_scalars_and_both_nested_offset_alternatives() {
    assert_field_wire_types::<DateTime>(&[8, 9], &[1, 2, 3, 4, 5, 6, 7]);
    assert_embedded_validation::<DateTime>(&[8, 9]);
    assert_field_wire_types::<TimeZone>(&[1, 2], &[]);
}

macro_rules! schema_wire_contracts {
    ($name:ident, $schema:ident) => {
        mod $name {
            use super::*;
            use $schema as schema;

            #[test]
            fn definitions_validate_wire_types_and_malformed_duplicate_submessages() {
                assert_field_wire_types::<schema::Schedule>(&[1, 2, 3, 4], &[]);
                assert_embedded_validation::<schema::Schedule>(&[1, 2, 3, 4]);
                assert_field_wire_types::<schema::schedule::At>(&[1], &[]);
                assert_embedded_validation::<schema::schedule::At>(&[1]);
                assert_field_wire_types::<schema::schedule::Every>(&[1], &[]);
                assert_embedded_validation::<schema::schedule::Every>(&[1]);
                assert_field_wire_types::<schema::schedule::Cron>(&[1, 2], &[]);
                assert_embedded_validation::<schema::schedule::Cron>(&[2]);
                assert_field_wire_types::<schema::schedule::RRule>(&[1, 2, 3, 4, 5], &[]);
                assert_embedded_validation::<schema::schedule::RRule>(&[1, 3, 4, 5]);
                assert_field_wire_types::<schema::Delivery>(&[1], &[]);
                assert_embedded_validation::<schema::Delivery>(&[1]);
                assert_field_wire_types::<schema::delivery::NatsMessage>(&[1, 2, 3], &[]);
                assert_embedded_validation::<schema::delivery::NatsMessage>(&[2, 3]);
                assert_field_wire_types::<schema::delivery::nats_message::Source>(&[1], &[]);
                assert_embedded_validation::<schema::delivery::nats_message::Source>(&[1]);
                assert_field_wire_types::<schema::delivery::nats_message::LatestFromSubject>(&[1], &[]);
                assert_field_wire_types::<schema::Message>(&[1, 2], &[]);
                assert_embedded_validation::<schema::Message>(&[1, 2]);
                assert_field_wire_types::<schema::Header>(&[1, 2], &[]);
            }
        }
    };
}

schema_wire_contracts!(live_tests, v1);
schema_wire_contracts!(checkpoint_tests, checkpoints_v1);
schema_wire_contracts!(projection_tests, projections_v1);

#[test]
fn lifecycle_and_storage_envelopes_validate_all_wire_fields() {
    assert_field_wire_types::<v1::CreateSchedule>(&[1, 2, 3, 4, 5], &[]);
    assert_embedded_validation::<v1::CreateSchedule>(&[2, 3, 4, 5]);
    assert_field_wire_types::<v1::ScheduleCreated>(&[1, 2, 3, 4, 5], &[]);
    assert_embedded_validation::<v1::ScheduleCreated>(&[2, 3, 4, 5]);
    assert_field_wire_types::<v1::ScheduleEvent>(&[1, 2, 3, 4, 5, 6, 7], &[]);
    assert_embedded_validation::<v1::ScheduleEvent>(&[1, 2, 3, 4, 5, 6, 7]);
    assert_field_wire_types::<v1::ScheduleStatus>(&[1, 2], &[]);
    assert_embedded_validation::<v1::ScheduleStatus>(&[1, 2]);
    assert_field_wire_types::<v1::PauseSchedule>(&[1], &[]);
    assert_field_wire_types::<v1::ResumeSchedule>(&[1], &[]);
    assert_field_wire_types::<v1::RemoveSchedule>(&[1], &[]);
    assert_field_wire_types::<v1::SchedulePaused>(&[1], &[]);
    assert_field_wire_types::<v1::ScheduleResumed>(&[1], &[]);
    assert_field_wire_types::<v1::ScheduleRemoved>(&[1], &[]);
    assert_field_wire_types::<v1::ScheduleOccurrenceScheduled>(&[1, 3, 4], &[2]);
    assert_embedded_validation::<v1::ScheduleOccurrenceScheduled>(&[3, 4]);
    assert_field_wire_types::<v1::ScheduleOccurrenceRecorded>(&[1, 3, 4], &[2]);
    assert_embedded_validation::<v1::ScheduleOccurrenceRecorded>(&[3, 4]);
    assert_field_wire_types::<v1::ScheduleCompleted>(&[1], &[2]);
    assert_field_wire_types::<checkpoints_v1::ScheduleCheckpoint>(&[1, 4, 6, 7, 8], &[2, 3, 5]);
    assert_embedded_validation::<checkpoints_v1::ScheduleCheckpoint>(&[6, 7, 8]);
    assert_field_wire_types::<projections_v1::ScheduleProjection>(&[1, 2, 4, 5, 6, 7, 8], &[3]);
    assert_embedded_validation::<projections_v1::ScheduleProjection>(&[2, 4, 5, 6, 7, 8]);
    assert_field_wire_types::<projections_v1::ScheduleStatus>(&[1, 2], &[]);
    assert_embedded_validation::<projections_v1::ScheduleStatus>(&[1, 2]);
    assert_field_wire_types::<state_v1::State>(&[2, 4, 5], &[1, 3, 6]);
    assert_embedded_validation::<state_v1::State>(&[2, 4, 5]);
}
