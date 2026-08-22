//! End-to-end execution test proving a multi-step `Decision::act()` chain crosses the real
//! WIT/wasmtime boundary and that later steps observe state evolved by earlier steps' events.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use trogon_decider_runtime::{
    AppendStreamRequest, AppendStreamResponse, ReadFrom, ReadStreamRequest, ReadStreamResponse, StreamAppend,
    StreamEvent, StreamPosition, StreamRead,
};
use trogon_decider_wasm_runtime::{WasmCommandExecution, WasmDeciderEngine, WasmDeciderModule, WasmEngineConfig};
use trogon_decider_wit::host::CommandEnvelope;

const STREAM_ID: &str = "act-chain-stream-1";
const RUN_TWO_STEP_PLAN_TYPE_URL: &str =
    "type.googleapis.com/trogon.decider.wasm_runtime.fixtures.act_chain.v1.RunTwoStepPlan";
const STEP_ONE_EVENT_TYPE: &str = "trogon.decider.wasm_runtime.fixtures.act_chain.v1.StepOneApplied";
const STEP_TWO_EVENT_TYPE: &str = "trogon.decider.wasm_runtime.fixtures.act_chain.v1.StepTwoApplied";

#[derive(Default)]
struct InMemoryEventStore {
    events: Mutex<Vec<StreamEvent>>,
}

impl InMemoryEventStore {
    fn stored_event_types(&self, stream_id: &str) -> Vec<String> {
        self.lock()
            .iter()
            .filter(|event| event.stream_id == stream_id)
            .map(|event| event.event.r#type.clone())
            .collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<StreamEvent>> {
        self.events.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn current_position(events: &[StreamEvent], stream_id: &str) -> Option<StreamPosition> {
        events
            .iter()
            .filter(|event| event.stream_id == stream_id)
            .map(|event| event.stream_position)
            .max()
    }
}

impl StreamRead<str> for InMemoryEventStore {
    type Error = std::convert::Infallible;

    async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
        let events = self.lock();
        let from_sequence = match request.from {
            ReadFrom::Beginning => 1,
            ReadFrom::Position(position) => position.as_u64(),
        };
        Ok(ReadStreamResponse {
            current_position: Self::current_position(&events, request.stream_id),
            events: events
                .iter()
                .filter(|event| event.stream_id == request.stream_id)
                .filter(|event| event.stream_position.as_u64() >= from_sequence)
                .cloned()
                .collect(),
        })
    }
}

impl StreamAppend<str> for InMemoryEventStore {
    type Error = std::convert::Infallible;

    async fn append_stream(&self, request: AppendStreamRequest<'_, str>) -> Result<AppendStreamResponse, Self::Error> {
        let mut events = self.lock();
        let mut next_sequence = Self::current_position(&events, request.stream_id)
            .map(StreamPosition::as_u64)
            .unwrap_or(0);
        for event in request.events {
            next_sequence += 1;
            events.push(StreamEvent {
                stream_id: request.stream_id.to_string(),
                event,
                stream_position: StreamPosition::try_new(next_sequence).expect("sequence starts at one"),
                recorded_at: chrono::Utc::now(),
            });
        }
        Ok(AppendStreamResponse {
            stream_position: StreamPosition::try_new(next_sequence).expect("append stores at least one event"),
        })
    }
}

fn act_chain_wasm() -> Vec<u8> {
    let relative = "../../../target/wasm32-unknown-unknown/release/trogon_act_chain_decider.wasm";
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "build trogon_act_chain_decider.wasm for wasm32-unknown-unknown first (expected {}): {error}",
            path.display()
        )
    })
}

fn act_chain_module() -> WasmDeciderModule {
    let engine = WasmDeciderEngine::new(WasmEngineConfig::default()).expect("engine builds");
    WasmDeciderModule::load(engine, &act_chain_wasm()).expect("module loads")
}

fn run_two_step_plan_command(stream_id: &str) -> CommandEnvelope {
    let stream_id_bytes = stream_id.as_bytes();
    let mut payload = vec![0x0A];
    let mut length = stream_id_bytes.len() as u64;
    loop {
        let mut byte = (length & 0x7F) as u8;
        length >>= 7;
        if length != 0 {
            byte |= 0x80;
        }
        payload.push(byte);
        if length == 0 {
            break;
        }
    }
    payload.extend_from_slice(stream_id_bytes);
    CommandEnvelope {
        type_: RUN_TWO_STEP_PLAN_TYPE_URL.to_string(),
        payload,
    }
}

fn position(value: u64) -> StreamPosition {
    StreamPosition::try_new(value).expect("test stream position must be non-zero")
}

#[tokio::test]
async fn a_two_step_act_chain_crosses_the_wasm_boundary_and_threads_state_between_steps() {
    let module = act_chain_module();
    let event_store = InMemoryEventStore::default();

    let result = WasmCommandExecution::new(&module, &event_store, &run_two_step_plan_command(STREAM_ID))
        .execute()
        .await
        .expect("the two-step act chain succeeds");

    assert_eq!(result.stream_position, position(2));
    assert_eq!(result.events.len(), 2);
    assert_eq!(result.events[0].r#type, STEP_ONE_EVENT_TYPE);
    assert!(result.events[0].content.is_empty());
    assert_eq!(result.events[1].r#type, STEP_TWO_EVENT_TYPE);
    assert_eq!(
        result.events[1].content,
        1u32.to_le_bytes().to_vec(),
        "the second step must observe state already evolved by the first step's event"
    );
    assert_eq!(
        event_store.stored_event_types(STREAM_ID),
        vec![STEP_ONE_EVENT_TYPE.to_string(), STEP_TWO_EVENT_TYPE.to_string()]
    );
}
