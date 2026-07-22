#![allow(unused_imports)]

use trogon_decider_guest_sdk::export_decider;
use trogon_scheduler_domain::CreateSchedule;
use trogonai_proto::scheduler::schedules::{CREATE_SCHEDULE_TYPE_URL, SCHEDULES_STATE_SCHEMA_VERSION, v1};

export_decider!(
    CreateSchedule {
        type_url = CREATE_SCHEDULE_TYPE_URL,
        proto = v1::CreateSchedule,
        module = "scheduler.schedules",
        version = "0.1.0",
        state_schema_version = SCHEDULES_STATE_SCHEMA_VERSION,
    },
    CreateSchedule {
        type_url = "type.googleapis.com/trogonai.scheduler.schedules.v1.Second",
        proto = v1::CreateSchedule,
        module = "scheduler.schedules",
        version = "0.2.0",
        state_schema_version = SCHEDULES_STATE_SCHEMA_VERSION,
    },
);

fn main() {}
