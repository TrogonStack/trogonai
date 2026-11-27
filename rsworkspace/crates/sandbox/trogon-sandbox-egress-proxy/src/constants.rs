use std::time::Duration;

/// The idle gap between requests that a kept-alive connection advertises.
pub const KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(5);

/// Bounds how many bytes are buffered while attempting to sniff a ClientHello,
/// so non-TLS or adversarial input cannot force unbounded buffering.
pub const MAX_SNIFF_BYTES: usize = 16 * 1024;

/// Bounds how long sniffing waits for more bytes before giving up and treating
/// the connection as having no determinable SNI.
pub const SNIFF_TIMEOUT: Duration = Duration::from_millis(500);

pub(crate) const TLS_DEFAULT_PORT: u16 = 443;

pub(crate) const MAX_HEAD_BYTES: usize = 64 * 1024;

pub(crate) const MAX_CHUNK_LINE_BYTES: usize = 1024;

pub(crate) const MAX_TRAILER_BYTES: usize = 8 * 1024;

pub(crate) const READ_CHUNK_BYTES: usize = 8192;
