use std::time::Duration;

/// `o5`: how long the first stream event waits for the ones that follow.
pub(super) const WINDOW: Duration = Duration::from_millis(100);

/// `ea`: the most a live-view row may weigh once serialized; a heavier one is dropped.
pub(super) const EPHEMERAL_LIMIT: usize = 61_440;

/// `MG`: the most a merged delta may weigh, which leaves room for the row around it.
pub(super) const MERGE_LIMIT: usize = EPHEMERAL_LIMIT - 4096;

/// `$y`: how large a live-only reply may be, in bytes.
pub(super) const LIVE_ONLY_LIMIT: usize = 57_344;

pub(super) const OVERSIZED_REPLY: &str = "The reply is too large to be sent without storing it";

/// No character takes more than this many bytes once escaped, so a batch of this many characters or fewer is safe
/// to merge without measuring.
pub(super) const WIDEST_ESCAPE: usize = 6;
