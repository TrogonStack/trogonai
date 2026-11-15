use std::num::{NonZeroU32, NonZeroU64};
use trogon_decider_runtime::{ConflictRetryLimit, ReplayLimit};

pub(crate) const PEER_REPLAY_LIMIT: ReplayLimit = ReplayLimit::new(NonZeroU64::MIN.saturating_add(4095));
pub(crate) const CONFLICT_RETRY_LIMIT: ConflictRetryLimit = ConflictRetryLimit::new(NonZeroU32::MIN);
pub(crate) const MAX_HISTORY_EVENTS: u64 = PEER_REPLAY_LIMIT.as_u64();
pub(crate) const DEFAULT_ROSTER_PAGE: u32 = 32;
pub(crate) const MAX_WORK: usize = 64;
pub(crate) const MAX_PROMPT_ID_BYTES: usize = 128;
pub(crate) const MAX_BATCH_ID_BYTES: usize = 128;
pub(crate) const MAX_TIMESTAMP_NANOS: i32 = 999_999_999;
