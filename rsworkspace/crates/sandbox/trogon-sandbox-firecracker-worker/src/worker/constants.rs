use std::time::Duration;

/// How often the worker republishes its capacity.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

/// Encoding room reserved for both streams' truncation markers: a marker is under 64 bytes, and the number-array
/// encoding costs up to four bytes per byte.
pub(super) const MARKER_ROOM: usize = 2 * 64 * 4;
