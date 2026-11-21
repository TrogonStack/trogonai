use crate::constants::MAX_FRAME_BYTES;
use std::time::Duration;

pub(super) const QUEUED_DATA_LIMIT: usize = 128 * 1024;
pub(super) const QUEUED_CONTROL_LIMIT: usize = 2 * MAX_FRAME_BYTES;
pub(super) const CLEANUP_NOTICE: Duration = Duration::from_secs(10);
pub(super) const ACK_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const INIT_TIMEOUT: Duration = Duration::from_secs(30);
