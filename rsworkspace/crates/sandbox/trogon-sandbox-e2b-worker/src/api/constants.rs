use std::time::Duration;

pub const DEFAULT_API_URL: &str = "https://api.e2b.app";
pub const DEFAULT_DOMAIN: &str = "e2b.app";

pub(crate) const MAX_TIMEOUT_SECS: u32 = 86_400;

/// How long a connection may take to establish.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Header the API key travels in.
pub(crate) const API_KEY_HEADER: &str = "X-API-Key";

/// Longest sandbox id accepted into a request path.
pub(crate) const MAX_ID_BYTES: usize = 128;

/// Longest provider message kept from an error body, in characters.
pub(crate) const MAX_ERROR_MESSAGE_CHARS: usize = 300;

/// Largest snapshot create reply accepted.
pub(crate) const MAX_CREATE_SNAPSHOT_REPLY_BYTES: usize = 16 * 1024;

/// Largest template reply accepted when resolving a snapshot.
pub(crate) const MAX_RESOLVE_SNAPSHOT_REPLY_BYTES: usize = 128 * 1024;

/// Suffix of the identity E2B returns for an unnamed snapshot.
pub(crate) const UNNAMED_SNAPSHOT_SUFFIX: &str = ":default";

/// Build status of a template whose snapshot can be restored.
pub(crate) const READY_BUILD_STATUS: &str = "ready";

/// HTTP status reported for an id that cannot be placed in a request path.
pub(crate) const INVALID_ID_STATUS: u16 = 400;
