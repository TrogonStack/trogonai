/// `onClose` when the worker could not be initialised for good (`4091`).
pub const CLOSE_INIT_FAILED: u16 = 4091;

/// `$y`: how large a live-only reply may be, in bytes.
pub(super) const LIVE_ONLY_LIMIT: usize = 57_344;

pub(super) const OVERSIZED_REPLY: &str = "The reply is too large to be sent without storing it";
