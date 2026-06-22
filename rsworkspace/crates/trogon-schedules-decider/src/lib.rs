//! Scheduler schedules WASM guest — four-command bundle.

use trogon_decider_guest_sdk::export_decider;
use trogon_scheduler_domain::{CreateSchedule, PauseSchedule, RemoveSchedule, ResumeSchedule};
use trogonai_proto::scheduler::schedules::{
    CREATE_SCHEDULE_TYPE_URL, PAUSE_SCHEDULE_TYPE_URL, REMOVE_SCHEDULE_TYPE_URL, RESUME_SCHEDULE_TYPE_URL,
    SCHEDULES_STATE_SCHEMA_VERSION, v1,
};

export_decider!(
    CreateSchedule {
        type_url = CREATE_SCHEDULE_TYPE_URL,
        proto = v1::CreateScheduleCommand,
        module = "scheduler.schedules",
        version = "0.1.0",
        state_schema_version = SCHEDULES_STATE_SCHEMA_VERSION,
        write_precondition = no_stream,
    },
    PauseSchedule {
        type_url = PAUSE_SCHEDULE_TYPE_URL,
        proto = v1::PauseScheduleCommand,
        module = "scheduler.schedules",
        version = "0.1.0",
        state_schema_version = SCHEDULES_STATE_SCHEMA_VERSION,
    },
    RemoveSchedule {
        type_url = REMOVE_SCHEDULE_TYPE_URL,
        proto = v1::RemoveScheduleCommand,
        module = "scheduler.schedules",
        version = "0.1.0",
        state_schema_version = SCHEDULES_STATE_SCHEMA_VERSION,
    },
    ResumeSchedule {
        type_url = RESUME_SCHEDULE_TYPE_URL,
        proto = v1::ResumeScheduleCommand,
        module = "scheduler.schedules",
        version = "0.1.0",
        state_schema_version = SCHEDULES_STATE_SCHEMA_VERSION,
    },
);
