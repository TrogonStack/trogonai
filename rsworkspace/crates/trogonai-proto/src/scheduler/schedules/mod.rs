mod codec;

// Thin wrappers that re-export the generated proto packages, emitted as inline
// module trees that mirror the codegen layout.
#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod checkpoints_v1 {
    pub use crate::r#gen::trogonai::scheduler::schedules::checkpoints::v1::*;
}

#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod projections_v1 {
    pub use crate::r#gen::trogonai::scheduler::schedules::projections::v1::*;
}

#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod state_v1 {
    pub use crate::r#gen::trogonai::scheduler::schedules::state::v1::*;
}

#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod v1 {
    pub use crate::r#gen::trogonai::scheduler::schedules::v1::*;
}

pub use codec::{ScheduleEventPayloadError, StateSnapshotPayloadError};
pub use v1::__buffa::oneof::delivery::Kind as DeliveryKind;
pub use v1::__buffa::oneof::delivery::nats_message::source::Kind as SourceKind;
pub use v1::__buffa::oneof::schedule::Kind as ScheduleKind;
pub use v1::__buffa::oneof::schedule_event::Event as ScheduleEventCase;
pub use v1::__buffa::oneof::schedule_status::Kind as ScheduleStatusKind;

/// Stable type URLs for scheduler command envelopes.
pub const CREATE_SCHEDULE_TYPE_URL: &str = v1::CreateSchedule::TYPE_URL;
pub const PAUSE_SCHEDULE_TYPE_URL: &str = v1::PauseSchedule::TYPE_URL;
pub const REMOVE_SCHEDULE_TYPE_URL: &str = v1::RemoveSchedule::TYPE_URL;
pub const RESUME_SCHEDULE_TYPE_URL: &str = v1::ResumeSchedule::TYPE_URL;

/// Stable type URL for the [`state_v1::State`] snapshot schema version tag.
pub const SCHEDULES_STATE_SCHEMA_VERSION: &str = state_v1::State::TYPE_URL;
