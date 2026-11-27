pub const MAX_REQUEST_BODY: u64 = 32 << 20;

pub const MAX_REPLY_FRAME: usize = 1 << 20;

pub(crate) const MAX_HEADER_ENTRIES: usize = 190;

pub(crate) const MAX_HEADER_BYTES: usize = 64 << 10;

pub(crate) const MAX_REMOVE_NAMES: usize = 190;

pub(crate) const MAX_HOST_PATH: usize = 8 << 10;

pub(crate) const MAX_REASON_BYTES: usize = 1 << 10;

pub(crate) const MAX_CREDENTIAL: usize = 8192;

pub(crate) const CREDENTIAL_HEADERS: [&str; 4] = ["authorization", "proxy-authorization", "cookie", "x-api-key"];

pub(crate) const FRAMING_HEADERS: [&str; 9] = [
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "upgrade",
    "te",
    "trailer",
    "keep-alive",
    "proxy-connection",
];
