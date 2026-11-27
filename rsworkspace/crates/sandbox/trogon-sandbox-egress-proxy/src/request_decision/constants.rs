pub const DEFAULT_DENY_REASON: &str = "denied by sandbox policy";

pub(super) const UNSETTABLE_HEADERS: [&str; 9] = [
    "host",
    "connection",
    "keep-alive",
    "proxy-connection",
    "transfer-encoding",
    "te",
    "trailer",
    "upgrade",
    "content-length",
];
