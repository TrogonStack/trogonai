use trogon_std::NowV7;
use uuid::Uuid;

use super::*;
use crate::WasmEngineConfig;
use crate::test_fixture::schedules_bytes;

fn position(value: u64) -> StreamPosition {
    StreamPosition::try_new(value).expect("test stream position must be non-zero")
}

#[derive(Debug, Clone, Copy)]
struct FixedUuidGenerator(Uuid);

impl NowV7 for FixedUuidGenerator {
    fn now_v7(&self) -> Uuid {
        self.0
    }
}

#[test]
fn spec_precondition_wins_over_override_and_position() {
    let resolved = resolve_write_precondition(
        Some(host::WritePrecondition::NoStream),
        Some(StreamWritePrecondition::Any),
        Some(position(7)),
    );
    assert_eq!(resolved, StreamWritePrecondition::NoStream);
}

#[test]
fn override_wins_over_observed_position() {
    let resolved = resolve_write_precondition(None, Some(StreamWritePrecondition::StreamExists), Some(position(7)));
    assert_eq!(resolved, StreamWritePrecondition::StreamExists);
}

#[test]
fn observed_position_is_the_fallback() {
    let resolved = resolve_write_precondition(None, None, Some(position(7)));
    assert_eq!(resolved, StreamWritePrecondition::At(position(7)));
}

#[test]
fn missing_position_falls_back_to_no_stream() {
    let resolved = resolve_write_precondition(None, None, None);
    assert_eq!(resolved, StreamWritePrecondition::NoStream);
}

#[test]
fn configured_no_stream_uses_the_fast_path() {
    assert!(has_no_stream_write_precondition(
        Some(host::WritePrecondition::NoStream),
        None
    ));
    assert!(has_no_stream_write_precondition(
        None,
        Some(StreamWritePrecondition::NoStream)
    ));
    assert!(!has_no_stream_write_precondition(
        Some(host::WritePrecondition::Any),
        Some(StreamWritePrecondition::NoStream)
    ));
    assert!(!has_no_stream_write_precondition(None, None));
}

#[test]
fn wit_preconditions_map_onto_stream_preconditions() {
    assert_eq!(
        to_stream_write_precondition(host::WritePrecondition::Any),
        StreamWritePrecondition::Any
    );
    assert_eq!(
        to_stream_write_precondition(host::WritePrecondition::StreamExists),
        StreamWritePrecondition::StreamExists
    );
    assert_eq!(
        to_stream_write_precondition(host::WritePrecondition::NoStream),
        StreamWritePrecondition::NoStream
    );
}

#[test]
fn snapshot_at_or_behind_stream_is_accepted() {
    assert!(ensure_snapshot_not_ahead(position(3), Some(position(3))).is_ok());
    assert!(ensure_snapshot_not_ahead(position(3), Some(position(9))).is_ok());
}

#[test]
fn snapshot_ahead_of_stream_is_rejected() {
    let Err(error) = ensure_snapshot_not_ahead(position(9), Some(position(3))) else {
        panic!("expected snapshot ahead of stream error");
    };
    assert_eq!(error.snapshot_position, position(9));
    assert_eq!(error.stream_position, Some(position(3)));

    assert!(ensure_snapshot_not_ahead(position(1), None).is_err());
}

#[test]
fn encode_events_assigns_host_ids_and_headers() {
    let id = Uuid::now_v7();
    let headers = Headers::empty();
    let envelopes = vec![AnyEnvelope {
        type_: "test.v1.Happened".to_string(),
        payload: vec![1, 2, 3],
    }];

    let events = encode_events(envelopes, &headers, &FixedUuidGenerator(id));

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, EventId::new(id));
    assert_eq!(events[0].r#type, "test.v1.Happened");
    assert_eq!(events[0].content, vec![1, 2, 3]);
    assert_eq!(events[0].headers, headers);
}

#[test]
fn replay_fuel_scales_linearly_with_event_count() {
    assert_eq!(replay_fuel(10, 1), 10);
    assert_eq!(replay_fuel(10, 250), 2_500);
}

#[test]
fn replay_fuel_saturates_instead_of_overflowing() {
    assert_eq!(replay_fuel(u64::MAX, 2), u64::MAX);
    assert_eq!(replay_fuel(u64::MAX, usize::MAX), u64::MAX);
}

#[test]
fn replay_epoch_ticks_scales_linearly_with_event_count() {
    assert_eq!(replay_epoch_ticks(10, 1), 10);
    assert_eq!(replay_epoch_ticks(10, 250), 2_500);
}

#[test]
fn replay_epoch_ticks_saturates_instead_of_overflowing() {
    assert_eq!(replay_epoch_ticks(u64::MAX, 2), u64::MAX);
    assert_eq!(replay_epoch_ticks(u64::MAX, usize::MAX), u64::MAX);
}

#[test]
fn a_zero_tick_epoch_deadline_traps_as_deadline_exceeded() {
    let engine = WasmDeciderEngine::new(WasmEngineConfig::default()).expect("engine builds");
    let module = WasmDeciderModule::load(engine, &schedules_bytes()).expect("module loads");

    let engine = module.engine();
    let mut store = engine.new_store();
    let command = CommandEnvelope {
        type_: "test.v1.DoesNotMatter".to_string(),
        payload: Vec::new(),
    };
    let context = GuestPhaseContext::new(&module, &command);
    let bindings = instantiate::<std::convert::Infallible, std::convert::Infallible, std::convert::Infallible>(
        &mut store,
        module.decider_pre(),
        engine,
        &context,
    )
    .expect("instantiate succeeds");

    // Arm the deadline directly and call the raw wit binding rather than the
    // `call_stream_id` wrapper above: the wrapper re-arms the deadline from
    // `engine.config().epoch_ticks_per_call()` immediately before its guest
    // call, which would silently clobber the zero-tick deadline this test
    // needs to force a trap.
    engine
        .arm_guest_call(&mut store, engine.config().fuel_per_call(), 0)
        .expect("arming a zero-tick deadline succeeds");
    let wasmtime_error =
        host::call_stream_id(&bindings, &mut store, &command).expect_err("a zero-tick deadline is already elapsed");
    let error: WasmCommandError<std::convert::Infallible, std::convert::Infallible, std::convert::Infallible> =
        map_trap(wasmtime_error, WasmCommandError::Trap);

    assert!(matches!(error, WasmCommandError::DeadlineExceeded(_)), "{error}");
}

#[test]
fn map_trap_distinguishes_epoch_deadline_from_other_traps() {
    let deadline_error: WasmCommandError<std::convert::Infallible, std::convert::Infallible, std::convert::Infallible> =
        map_trap(wasmtime::Error::from(wasmtime::Trap::Interrupt), WasmCommandError::Trap);
    assert!(matches!(deadline_error, WasmCommandError::DeadlineExceeded(_)));

    let other_error: WasmCommandError<std::convert::Infallible, std::convert::Infallible, std::convert::Infallible> =
        map_trap(wasmtime::Error::from(wasmtime::Trap::OutOfFuel), WasmCommandError::Trap);
    assert!(matches!(other_error, WasmCommandError::Trap(_)));
}

#[test]
fn rejected_decide_errors_stay_rejections() {
    let error: WasmCommandError<std::convert::Infallible, std::convert::Infallible, std::convert::Infallible> =
        map_decide_error(DecideError::Rejected(host::DomainError {
            code: "already-exists".to_string(),
            message: "schedule already exists".to_string(),
            details: Vec::new(),
        }));

    let WasmCommandError::Rejected(detail) = error else {
        panic!("expected rejected error");
    };
    assert_eq!(detail.code, "already-exists");
}

#[test]
fn faulted_decide_errors_stay_faults() {
    let error: WasmCommandError<std::convert::Infallible, std::convert::Infallible, std::convert::Infallible> =
        map_decide_error(DecideError::Faulted(host::DomainError {
            code: "decode-failed".to_string(),
            message: "payload did not decode".to_string(),
            details: Vec::new(),
        }));

    let WasmCommandError::Faulted(detail) = error else {
        panic!("expected faulted error");
    };
    assert_eq!(detail.code, "decode-failed");
}

#[test]
fn stream_events_project_onto_guest_envelopes() {
    let stream_event = trogon_decider_runtime::StreamEvent {
        stream_id: "backup".to_string(),
        event: Event {
            id: EventId::new(Uuid::now_v7()),
            r#type: "test.v1.Happened".to_string(),
            content: vec![4, 5, 6],
            headers: Headers::empty(),
        },
        stream_position: position(1),
        recorded_at: chrono::Utc::now(),
    };

    let envelopes = to_any_envelopes(vec![stream_event]);

    assert_eq!(envelopes.len(), 1);
    assert_eq!(envelopes[0].type_, "test.v1.Happened");
    assert_eq!(envelopes[0].payload, vec![4, 5, 6]);
}
