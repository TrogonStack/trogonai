//! Light decider WASM guest — counter example for M0/M1.

use trogon_decider_guest_sdk::export_decider;
use trogonai_proto::example::{LIGHT_STATE_SCHEMA_VERSION, TURN_ON_TYPE_URL, TurnOnCommand};

export_decider!(TurnOnCommand {
    type_url = TURN_ON_TYPE_URL,
    proto = trogonai_proto::example::v1::TurnOn,
    module = "example.light",
    version = "0.1.0",
    state_schema_version = LIGHT_STATE_SCHEMA_VERSION,
    write_precondition = no_stream,
});
