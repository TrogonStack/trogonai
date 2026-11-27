use std::time::Duration;

pub(super) const HTTP_DEFAULT_PORT: u16 = 80;

pub(super) const MAX_DRAIN_BYTES: usize = 1024 * 1024;

pub(super) const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

pub(super) const HOP_BY_HOP: &[&str] = &[
    "proxy-authorization",
    "proxy-authenticate",
    "proxy-connection",
    "connection",
    "keep-alive",
    "upgrade",
    "te",
    "trailer",
    "transfer-encoding",
];
