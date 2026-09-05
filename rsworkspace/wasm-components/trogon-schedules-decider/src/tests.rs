use buffa::{Message as _, MessageField, MessageName as _};
use buffa_types::google::protobuf::Duration;
use trogon_decider::Decider;

use super::*;
use __trogon_decider_bindings::{CommandEnvelope, DecideError, Guest, GuestSession, SnapshotPolicy, WritePrecondition};

#[test]
fn descriptor_exports_the_native_commands_and_snapshot_policies() {
    let descriptor = Component::descriptor();
    assert_eq!(descriptor.name, "scheduler.schedules");
    assert_eq!(descriptor.commands.len(), 4);
    let expected = [
        (CREATE_SCHEDULE_TYPE_URL, CreateSchedule::SNAPSHOT_CADENCE),
        (PAUSE_SCHEDULE_TYPE_URL, PauseSchedule::SNAPSHOT_CADENCE),
        (REMOVE_SCHEDULE_TYPE_URL, RemoveSchedule::SNAPSHOT_CADENCE),
        (RESUME_SCHEDULE_TYPE_URL, ResumeSchedule::SNAPSHOT_CADENCE),
    ];
    assert!(matches!(
        descriptor.commands[0].write_precondition,
        WritePrecondition::NoStream
    ));
    for command in &descriptor.commands[1..] {
        assert!(matches!(command.write_precondition, WritePrecondition::StreamUnchanged));
    }
    for (command, (type_url, cadence)) in descriptor.commands.iter().zip(expected) {
        assert_eq!(command.command_type, type_url);
        let frequency = match command.snapshot_policy {
            SnapshotPolicy::NoSnapshot => None,
            SnapshotPolicy::Frequency(frequency) => Some(frequency),
        };
        assert_eq!(frequency, cadence.frequency().map(std::num::NonZeroU64::get));
    }
}

fn create_envelope(id: &str) -> CommandEnvelope {
    let command = v1::CreateSchedule {
        schedule_id: id.to_string(),
        status: MessageField::some(v1::ScheduleStatus {
            kind: Some(v1::schedule_status::Scheduled {}.into()),
        }),
        schedule: MessageField::some(v1::Schedule {
            kind: Some(
                v1::schedule::Every {
                    every: MessageField::some(Duration {
                        seconds: 30,
                        ..Default::default()
                    }),
                }
                .into(),
            ),
        }),
        delivery: MessageField::some(v1::Delivery {
            kind: Some(
                v1::delivery::NatsMessage {
                    subject: "agent.run".to_string(),
                    ..Default::default()
                }
                .into(),
            ),
        }),
        message: MessageField::some(v1::Message {
            content: MessageField::some(trogonai_proto::content::v1alpha1::Content {
                content_type: "application/json".to_string(),
                data: b"{}".to_vec(),
            }),
            headers: Vec::new(),
        }),
    };
    CommandEnvelope {
        type_: CREATE_SCHEDULE_TYPE_URL.to_string(),
        payload: command.encode_to_vec(),
    }
}

#[test]
fn exported_session_routes_the_bundle_and_restores_its_lifecycle_state() -> Result<(), DecideError> {
    let id = "0198fa2f6d0a7b1a8cf9f762e73a1c45";
    assert_eq!(
        Component::stream_id(create_envelope(id)).map_err(DecideError::Faulted)?,
        id
    );
    let session = Session::new(None);
    let initial = session.snapshot();
    let created = session.decide(create_envelope(id))?;
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].type_, v1::ScheduleCreated::FULL_NAME);
    assert_eq!(session.snapshot(), initial);
    session.evolve(created).map_err(DecideError::Faulted)?;
    let session = Session::new(session.snapshot());
    for (type_url, payload, event_type) in [
        (
            PAUSE_SCHEDULE_TYPE_URL,
            v1::PauseSchedule {
                schedule_id: id.to_string(),
            }
            .encode_to_vec(),
            v1::SchedulePaused::FULL_NAME,
        ),
        (
            RESUME_SCHEDULE_TYPE_URL,
            v1::ResumeSchedule {
                schedule_id: id.to_string(),
            }
            .encode_to_vec(),
            v1::ScheduleResumed::FULL_NAME,
        ),
        (
            REMOVE_SCHEDULE_TYPE_URL,
            v1::RemoveSchedule {
                schedule_id: id.to_string(),
            }
            .encode_to_vec(),
            v1::ScheduleRemoved::FULL_NAME,
        ),
    ] {
        assert_eq!(
            Component::stream_id(CommandEnvelope {
                type_: type_url.to_string(),
                payload: payload.clone(),
            })
            .map_err(DecideError::Faulted)?,
            id
        );
        let events = session.decide(CommandEnvelope {
            type_: type_url.to_string(),
            payload,
        })?;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].type_, event_type);
        session.evolve(events).map_err(DecideError::Faulted)?;
    }
    let removed = Session::new(session.snapshot());
    assert!(matches!(
        removed.decide(create_envelope(id)),
        Err(DecideError::Rejected(_))
    ));
    Ok(())
}

#[test]
fn exported_admission_failures_leave_the_session_unchanged() {
    let session = Session::new(None);
    let initial = session.snapshot();
    for (type_url, payload) in [
        ("future.scheduler.Command", Vec::new()),
        (PAUSE_SCHEDULE_TYPE_URL, vec![0xff]),
        (
            RESUME_SCHEDULE_TYPE_URL,
            v1::ResumeSchedule {
                schedule_id: "invalid".to_string(),
            }
            .encode_to_vec(),
        ),
    ] {
        assert!(
            Component::stream_id(CommandEnvelope {
                type_: type_url.to_string(),
                payload: payload.clone(),
            })
            .is_err()
        );
        assert!(matches!(
            session.decide(CommandEnvelope {
                type_: type_url.to_string(),
                payload,
            }),
            Err(DecideError::Faulted(_))
        ));
    }
    assert_eq!(session.snapshot(), initial);
}
