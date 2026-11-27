/// Request header names a `Forward` domain rule injects into a matched
/// request before it reaches `forward_url`, under this
/// project's `trogon-` prefix. `trogon-forwarded-*` carries the
/// connection's *original* (SNI-sniffed) destination, since `forward_url`
/// itself replaces the actual connection target and a receiving proxy
/// otherwise has no way to recover what the sandboxed workload actually
/// asked for.
pub(crate) const FORWARDED_HOST_HEADER: &str = "trogon-forwarded-host";

pub(crate) const FORWARDED_SCHEME_HEADER: &str = "trogon-forwarded-scheme";

pub(crate) const FORWARDED_PORT_HEADER: &str = "trogon-forwarded-port";

pub(crate) const FORWARDED_PATH_HEADER: &str = "trogon-forwarded-path";

pub(crate) const SANDBOX_OIDC_TOKEN_HEADER: &str = "trogon-sandbox-oidc-token";

pub(crate) const REQUEST_HEAD_LIMIT: usize = 16 * 1024;

/// the cap on the chunk extension bytes of one chunk
/// per request.
pub(super) const MAX_CHUNK_EXTENSION_BYTES: usize = 16 * 1024;

/// How long a request's chunk-size or trailer line may grow while it holds
/// no more counted bytes than the workload's HTTP stack allows.
pub(super) const MAX_REQUEST_BODY_LINE_BYTES: usize = 4 * REQUEST_HEAD_LIMIT;

/// What a strict HTTP parser measures trailers against, the way it measures a head
/// (`maxHeaderSize`).
pub(super) const MAX_REQUEST_TRAILER_BYTES: usize = 16 * 1024;

/// What a strict HTTP server reads off a client's socket at once
/// (`net` `kReadableHighWaterMark`).
pub(crate) const CLIENT_READ_BYTES: usize = 64 * 1024;

/// How much of what a client has already sent is read in before its request
/// goes upstream, so that a bad body that arrived with the head is refused
/// before anything is.
pub(super) const READY_BYTES_LIMIT: usize = 2 * CLIENT_READ_BYTES;
