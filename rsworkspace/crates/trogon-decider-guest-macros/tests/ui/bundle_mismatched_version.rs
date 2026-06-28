#![allow(unused_imports)]

use trogon_decider_guest_sdk::export_decider;
use trogonai_proto::example::{LIGHT_STATE_SCHEMA_VERSION, TURN_ON_TYPE_URL, TurnOnCommand, v1};

export_decider!(
    TurnOnCommand {
        type_url = TURN_ON_TYPE_URL,
        proto = v1::TurnOn,
        module = "example.light",
        version = "0.1.0",
        state_schema_version = LIGHT_STATE_SCHEMA_VERSION,
    },
    TurnOnCommand {
        type_url = "type.googleapis.com/trogonai.example.light.v1.Second",
        proto = v1::TurnOn,
        module = "example.light",
        version = "0.2.0",
        state_schema_version = LIGHT_STATE_SCHEMA_VERSION,
    },
);

fn main() {}
