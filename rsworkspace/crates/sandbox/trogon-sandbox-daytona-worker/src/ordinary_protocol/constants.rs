use std::time::Duration;

/// The protocol version both sides must speak.
pub(crate) const VERSION: u32 = 1;

/// How long the worker waits for the bridge to acknowledge one control frame.
pub(crate) const CONTROL_TIMEOUT: Duration = Duration::from_secs(15);

/// The largest state or acknowledgement file the worker accepts.
pub(crate) const MAX_STATE_BYTES: usize = 64 * 1024;

/// The largest control frame the worker sends.
pub(crate) const MAX_FRAME_BYTES: usize = 1024 * 1024;
