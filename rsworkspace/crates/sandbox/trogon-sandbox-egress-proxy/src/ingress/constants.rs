use std::time::Duration;

pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// The reference timeout, which bounds an HTTP request head.
pub const HEAD_TIMEOUT: Duration = Duration::from_secs(60);

/// The reference keep-alive buffer, added to the idle gap before it closes the
/// connection, though the `Keep-Alive` header advertises only the timeout.
pub const KEEP_ALIVE_GRACE: Duration = Duration::from_secs(1);

pub const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

pub const MAX_HEAD_BYTES: usize = 16 * 1024;

pub const PROXY_ONLY_MAX_HEAD_BYTES: usize = 2 * 1024 * 1024;
