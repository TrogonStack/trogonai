#[cfg(feature = "chrono")]
pub const PROTOBUF_DURATION_MAX_SECONDS: u64 = 315_576_000_000;
#[cfg(feature = "chrono")]
pub const PROTOBUF_DURATION_MAX: std::time::Duration = std::time::Duration::from_secs(PROTOBUF_DURATION_MAX_SECONDS);

#[cfg(feature = "schedules")]
use crate::scheduler::schedules::{state_v1, v1};

#[cfg(feature = "schedules")]
/// Stable type URLs for scheduler command envelopes.
pub const CREATE_SCHEDULE_TYPE_URL: &str = v1::CreateSchedule::TYPE_URL;
#[cfg(feature = "schedules")]
pub const PAUSE_SCHEDULE_TYPE_URL: &str = v1::PauseSchedule::TYPE_URL;
#[cfg(feature = "schedules")]
pub const REMOVE_SCHEDULE_TYPE_URL: &str = v1::RemoveSchedule::TYPE_URL;
#[cfg(feature = "schedules")]
pub const RESUME_SCHEDULE_TYPE_URL: &str = v1::ResumeSchedule::TYPE_URL;

#[cfg(feature = "schedules")]
/// Schema-version tag for the [`state_v1::State`] snapshot frame.
///
/// Uses the proto full name (not the `type.googleapis.com/` URL) so it matches the snapshot type
/// the native runtime resolves through its `SnapshotType` impl (`State::FULL_NAME`); a host can
/// therefore hand a native snapshot to the guest frame and have `decode_snapshot` accept it
/// instead of trapping on a tag mismatch.
pub const SCHEDULES_STATE_SCHEMA_VERSION: &str = <state_v1::State as buffa::MessageName>::FULL_NAME;
