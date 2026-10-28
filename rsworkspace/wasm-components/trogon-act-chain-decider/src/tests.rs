use buffa::{DecodeError, Message};
use trogon_decider::testing::TestCase;

use super::*;
use __trogon_decider_bindings::WritePrecondition as GuestWritePrecondition;
use __trogon_decider_bindings::{CommandEnvelope, DecideError, Guest, GuestSession, SnapshotPolicy};

#[test]
fn descriptor_keeps_the_act_commands_native_policies() {
    let descriptor = Component::descriptor();
    assert_eq!(descriptor.commands.len(), 1);
    let command = &descriptor.commands[0];
    assert_eq!(command.command_type, constants::RUN_TWO_STEP_PLAN_TYPE_URL);
    assert!(matches!(
        command.write_precondition,
        GuestWritePrecondition::StreamUnchanged
    ));
    assert!(matches!(command.snapshot_policy, SnapshotPolicy::NoSnapshot));
}

fn plan_envelope() -> CommandEnvelope {
    CommandEnvelope {
        type_: constants::RUN_TWO_STEP_PLAN_TYPE_URL.to_string(),
        payload: RunTwoStepPlan {
            stream_id: "plan-7".to_string(),
        }
        .encode_to_vec(),
    }
}

#[test]
fn exported_session_keeps_decisions_uncommitted_until_replay_and_restores_snapshots() -> Result<(), DecideError> {
    assert_eq!(
        Component::stream_id(plan_envelope()).map_err(DecideError::Faulted)?,
        "plan-7"
    );
    let session = Session::new(None);
    let initial = session.snapshot();
    let events = session.decide(plan_envelope())?;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].type_, constants::STEP_ONE_EVENT_TYPE);
    assert_eq!(events[1].type_, constants::STEP_TWO_EVENT_TYPE);
    assert_eq!(events[1].payload, 1_u32.to_le_bytes());
    assert_eq!(session.snapshot(), initial);

    session.evolve(events).map_err(DecideError::Faulted)?;
    let persisted = session.snapshot();
    assert!(persisted.is_some());
    let restored = Session::new(persisted);
    assert!(matches!(
        restored.decide(plan_envelope()),
        Err(DecideError::Rejected(_))
    ));
    assert_eq!(restored.snapshot(), session.snapshot());
    Ok(())
}

#[test]
fn exported_command_admission_rejects_unknown_types_and_malformed_payloads() {
    let session = Session::new(None);
    let initial = session.snapshot();
    for (type_url, payload) in [
        ("future.plan.Command", Vec::new()),
        (constants::RUN_TWO_STEP_PLAN_TYPE_URL, vec![0x08, 0x01]),
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

#[test]
fn the_second_step_observes_state_evolved_by_the_first() {
    let command = RunTwoStepPlan {
        stream_id: "plan-7".to_owned(),
    };
    assert_eq!(command.stream_id(), "plan-7");
    TestCase::<RunTwoStepPlan>::new()
        .given_no_history()
        .when(command)
        .then([
            PlanEvent::StepOneApplied,
            PlanEvent::StepTwoApplied {
                steps_observed_before: 1,
            },
        ]);
}

#[test]
fn replay_prevents_running_a_plan_again() {
    TestCase::<RunTwoStepPlan>::new()
        .given([PlanEvent::StepOneApplied])
        .when(RunTwoStepPlan {
            stream_id: "plan-7".to_owned(),
        })
        .then_error(PlanDecideError::AlreadyRan);
}

#[test]
fn command_and_snapshot_wire_match_the_wasm_host_contract() -> Result<(), DecodeError> {
    let command_wire = b"\x0a\x06plan-7";
    let mut command = RunTwoStepPlan::decode_from_slice(command_wire)?;
    assert_eq!(command.stream_id, "plan-7");
    assert_eq!(command.encode_to_vec(), command_wire);
    command.clear();
    assert_eq!(command.encode_to_vec(), b"\x0a\x00");

    let state_wire = b"\x08\x96\x01";
    let mut state = PlanState::decode_from_slice(state_wire)?;
    assert_eq!(state.steps_applied, 150);
    assert_eq!(state.encode_to_vec(), state_wire);
    state.clear();
    assert_eq!(state.encode_to_vec(), b"\x08\x00");
    Ok(())
}

#[test]
fn future_fields_are_ignored_and_duplicate_singular_fields_replace() -> Result<(), DecodeError> {
    let command = RunTwoStepPlan::decode_from_slice(b"\x10\x01\x0a\x03old\x0a\x03new")?;
    assert_eq!(command.stream_id, "new");
    let state = PlanState::decode_from_slice(b"\x12\x01x\x08\x01\x08\x02")?;
    assert_eq!(state.steps_applied, 2);
    Ok(())
}

#[test]
fn malformed_command_and_snapshot_wire_retain_decode_errors() {
    assert!(matches!(
        RunTwoStepPlan::decode_from_slice(b"\x08\x01"),
        Err(DecodeError::WireTypeMismatch {
            field_number: 1,
            expected: 2,
            actual: 0
        })
    ));
    assert_eq!(
        RunTwoStepPlan::decode_from_slice(b"\x0a\x01\xff").err(),
        Some(DecodeError::InvalidUtf8)
    );
    assert_eq!(
        RunTwoStepPlan::decode_from_slice(b"\x12\x02x").err(),
        Some(DecodeError::UnexpectedEof)
    );
    assert!(matches!(
        PlanState::decode_from_slice(b"\x0a\x00"),
        Err(DecodeError::WireTypeMismatch {
            field_number: 1,
            expected: 0,
            actual: 2
        })
    ));
    assert_eq!(
        PlanState::decode_from_slice(b"\x08\x80").err(),
        Some(DecodeError::UnexpectedEof)
    );
    assert_eq!(
        PlanState::decode_from_slice(b"\x12\x02x").err(),
        Some(DecodeError::UnexpectedEof)
    );
}

#[test]
fn plan_events_have_stable_type_names_and_little_endian_payloads() -> Result<(), StepTwoDecodeError> {
    for (event, event_type, payload) in [
        (PlanEvent::StepOneApplied, constants::STEP_ONE_EVENT_TYPE, Vec::new()),
        (
            PlanEvent::StepTwoApplied {
                steps_observed_before: 0x04030201,
            },
            constants::STEP_TWO_EVENT_TYPE,
            vec![1, 2, 3, 4],
        ),
    ] {
        assert_eq!(event.event_type(), Ok(event_type));
        assert_eq!(event.encode(), Ok(payload.clone()));
        assert!(
            matches!(PlanEvent::decode(EventData::new(event_type, &payload))?, EventDecodeOutcome::Decoded(decoded) if decoded == event)
        );
    }
    assert!(matches!(
        PlanEvent::decode(EventData::new("future.event", b"\xff"))?,
        EventDecodeOutcome::Skipped
    ));
    for payload in [b"".as_slice(), b"\x01\x02\x03", b"\x01\x02\x03\x04\x05"] {
        assert_eq!(
            PlanEvent::decode(EventData::new(constants::STEP_TWO_EVENT_TYPE, payload))
                .err()
                .map(|error| error.actual),
            Some(payload.len())
        );
    }
    Ok(())
}
