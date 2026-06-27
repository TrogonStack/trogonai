//! Smoke test: load spike wasm, call stream-id + decide.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::fs;

use buffa::Message as _;
use trogon_decider_wit::host::{self, CommandEnvelope, Decider};
use trogonai_proto::example::{TURN_ON_TYPE_URL, v1};
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store};

#[test]
fn smoke_light_spike() {
    let wasm_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../trogon-decider-spike/../../target/wasm32-unknown-unknown/release/trogon_decider_spike.wasm");
    let bytes = fs::read(&wasm_path).expect("build trogon-decider-spike for wasm32-unknown-unknown first");

    trogon_decider_sim::assert_zero_imports(&bytes).expect("zero imports");

    let mut config = Config::new();
    config.wasm_component_model(true);
    let engine = Engine::new(&config).unwrap();
    let component = Component::new(&engine, &bytes).unwrap();

    struct State;

    let linker = Linker::new(&engine);
    let mut store = Store::new(&engine, State);
    let bindings = Decider::instantiate(&mut store, &component, &linker).unwrap();

    let descriptor = host::call_descriptor(&bindings, &mut store).unwrap();
    assert_eq!(descriptor.name, "example.light");

    let command = CommandEnvelope {
        type_: TURN_ON_TYPE_URL.to_string(),
        payload: v1::TurnOn {
            light_id: "kitchen".to_string(),
        }
        .encode_to_vec(),
    };

    let stream_id = host::call_stream_id(&bindings, &mut store, &command).unwrap().unwrap();
    assert_eq!(stream_id, "kitchen");

    let session = host::create_session(&bindings, &mut store, None).unwrap();
    let events = host::decide(&bindings, &mut store, session, &command).unwrap().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].type_.contains("LightTurnedOn"));
}
