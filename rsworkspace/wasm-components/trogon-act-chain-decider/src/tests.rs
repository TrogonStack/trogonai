use buffa::{DecodeError, Message};
use trogon_decider::testing::TestCase;

use super::*;

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
