use std::time::Duration;

/// Bound on every OpenBao HTTP call a release read makes, so a stalled backend
/// fails the release before its caller stops waiting.
pub(super) const RELEASE_REQUEST_TIMEOUT: Duration = Duration::from_secs(1);

/// Bound on every OpenBao HTTP call a static-token store makes.
///
/// The gateway's credential lifecycle writes and reads keep the bound they had
/// before the release path existed, until the gateway resolves through the
/// secrets service instead.
pub(super) const STATIC_TOKEN_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The header that carries a release's audit correlation id into OpenBao.
///
/// OpenBao writes it to its audit log only once the header is registered under
/// `sys/config/auditing/request-headers`; the token metadata carries the same
/// id unconditionally.
pub(super) const CORRELATION_ID_HEADER: &str = "x-trogon-correlation-id";

/// The child token metadata key OpenBao logs as `auth.metadata` on every request
/// made with that token.
pub(super) const CORRELATION_ID_METADATA_KEY: &str = "correlation_id";

/// The OpenBao calls a release read makes while it depends on the login token:
/// minting the child, then the metadata and data reads the child makes before
/// the login token's revocation would sever it.
const RELEASE_READ_CALLS: u64 = 3;

/// How close to expiry a login token stops being sent.
///
/// A release read can run for up to one request timeout per call it makes, so a
/// token with less than that left could expire mid-read.
pub(super) const TOKEN_EXPIRY_MARGIN: Duration =
    Duration::from_secs(RELEASE_REQUEST_TIMEOUT.as_secs() * RELEASE_READ_CALLS);

/// The last part of a login token's lease, by fraction, in which it is renewed
/// rather than used as is.
pub(super) const RENEWAL_WINDOW_DIVISOR: u32 = 3;

/// The reads a child token is good for: the metadata read that fails closed on a
/// retired version, then the data read.
pub(super) const CHILD_TOKEN_USES: u32 = 2;

/// A child token outlives its reads only as a backstop; its use count ends it
/// first.
pub(super) const CHILD_TOKEN_TTL: Duration = Duration::from_secs(5);

/// The name a child token shows in OpenBao's audit log and token lookups.
pub(super) const CHILD_TOKEN_DISPLAY_NAME: &str = "credential-release";

/// Where an AppRole logs in, relative to the OpenBao address.
pub(super) const APPROLE_LOGIN_PATH: &str = "/v1/auth/approle/login";

/// Where a login token extends its own lease.
pub(super) const TOKEN_RENEW_SELF_PATH: &str = "/v1/auth/token/renew-self";

/// Where a child token is minted through a token role; the role name follows.
pub(super) const TOKEN_CREATE_PATH: &str = "/v1/auth/token/create";

/// The header OpenBao reads a request's token from.
pub(super) const TOKEN_HEADER: &str = "X-Vault-Token";
