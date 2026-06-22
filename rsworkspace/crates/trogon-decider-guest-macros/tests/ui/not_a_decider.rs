use trogon_decider_guest_sdk::export_decider;
use trogonai_proto::example::{TurnOnCommand, TURN_ON_TYPE_URL, LIGHT_STATE_SCHEMA_VERSION};

struct NotADecider;

export_decider!(NotADecider {
    type_url = TURN_ON_TYPE_URL,
    proto = trogonai_proto::example::v1::TurnOn,
    module = "example.light",
    version = "0.1.0",
    state_schema_version = LIGHT_STATE_SCHEMA_VERSION,
});

fn main() {}
