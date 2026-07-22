use trogon_decider_guest_sdk::export_decider;
use trogonai_proto::scheduler::schedules::{CREATE_SCHEDULE_TYPE_URL, SCHEDULES_STATE_SCHEMA_VERSION, v1};

struct NotADecider;

export_decider!(NotADecider {
    type_url = CREATE_SCHEDULE_TYPE_URL,
    proto = v1::CreateSchedule,
    module = "scheduler.schedules",
    version = "0.1.0",
    state_schema_version = SCHEDULES_STATE_SCHEMA_VERSION,
});

fn main() {}
