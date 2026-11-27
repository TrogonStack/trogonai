pub(super) const AUTHORIZATION: &str = "authorization";

pub(super) const AMZ_DATE: &str = "x-amz-date";

pub(super) const AMZ_SECURITY_TOKEN: &str = "x-amz-security-token";

pub(super) const AMZ_CONTENT_SHA256: &str = "x-amz-content-sha256";

pub(super) const CONNECTION: &str = "connection";

/// The headers the proxy drops before it forwards a request, apart from the
/// ones the request's own `Connection` header names.
pub(super) const HOP_BY_HOP: [&str; 9] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];
