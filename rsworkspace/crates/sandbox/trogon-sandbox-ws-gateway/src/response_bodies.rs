use std::time::SystemTime;

use serde::Serialize;
use trogon_sandbox_contract::LeaseReleasedReason;

/// A response that only names what happened, such as `stopped` or `applied`.
#[derive(Debug, Serialize)]
pub(crate) struct OutcomeBody {
    pub outcome: &'static str,
}

#[derive(Debug, Serialize)]
pub(crate) struct CancelledBody {
    pub cancelled: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct UpdatedBody {
    pub updated: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct DeletedBody {
    pub deleted: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct SubscribedBody {
    pub subscribed: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct DashboardSubscribedBody {
    pub subscribed: bool,
    pub streams: [&'static str; 3],
}

#[derive(Debug, Serialize)]
pub(crate) struct ReleasedBody {
    pub reason: LeaseReleasedReason,
}

#[derive(Debug, Serialize)]
pub(crate) struct DrivesBody<T> {
    pub drives: T,
}

#[derive(Debug, Serialize)]
pub(crate) struct ExtendedBody {
    pub outcome: &'static str,
    pub new_deadline: SystemTime,
}

#[derive(Debug, Serialize)]
pub(crate) struct FollowingBody {
    pub following: bool,
    pub stream_id: String,
}

/// The reply of `sandbox_read_file`.
#[derive(Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub(crate) enum ReadFileBody {
    Content { data: Vec<u8> },
    NotFound,
}
