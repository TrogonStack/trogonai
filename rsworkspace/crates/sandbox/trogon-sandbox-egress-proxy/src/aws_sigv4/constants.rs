pub(crate) const SIGV4_ALGORITHM: &str = "AWS4-HMAC-SHA256";

pub(crate) const SIGV4A_ALGORITHM: &str = "AWS4-ECDSA-P256-SHA256";

pub(crate) const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";

pub(super) const STREAMING_PREFIX: &str = "STREAMING-";

pub(super) const SCOPE_TERMINATOR: &str = "aws4_request";

pub(super) const HMAC_BLOCK: usize = 64;
