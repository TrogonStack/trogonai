#![allow(unused_imports)]

use trogon_decider_guest_sdk::export_decider;
use trogon_scheduler_domain::{CreateSchedule, PauseSchedule};
use trogonai_proto::scheduler::schedules::{
    CREATE_SCHEDULE_TYPE_URL, PAUSE_SCHEDULE_TYPE_URL, SCHEDULES_STATE_SCHEMA_VERSION, v1,
};

export_decider!(
    CreateSchedule {
        type_url = CREATE_SCHEDULE_TYPE_URL,
        proto = v1::CreateSchedule,
        module = "scheduler.schedules",
        version = "0.1.0",
        state_schema_version = SCHEDULES_STATE_SCHEMA_VERSION,
    },
    PauseSchedule {
        type_url = PAUSE_SCHEDULE_TYPE_URL,
        proto = v1::PauseSchedule,
        module = "scheduler.schedules",
        version = "0.1.0",
        state_schema_version = "type.googleapis.com/trogonai.scheduler.schedules.state.v2.State",
    },
);

fn main() {}
