use std::time::Duration;

pub const MAX_BODY_READ: usize = 32 << 20;

pub(super) const BODY_READ_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) const MAX_FRAMING_OVERHEAD: usize = 64 << 10;

pub(super) const MAX_CHUNK_LINE: usize = 4096;
