use std::time::Duration;

pub const MAX_REGISTRATION_LIFETIME: Duration = Duration::from_secs(330);
pub const MAX_SVID_LIFETIME: Duration = Duration::from_secs(3600);
pub const MAX_SPIFFE_ID_BYTES: usize = 2048;
pub const MAX_BINDINGS: usize = 256;
pub const MAX_ROOTS: usize = 16;
pub const MAX_IDENTITIES: usize = 32;
pub const MAX_CONTEXT_BYTES: usize = 1024 * 1024;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const RECONNECT_DELAY: Duration = Duration::from_millis(250);
pub const LIFECYCLE_POLL: Duration = Duration::from_millis(100);
pub const TEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Default ceiling on the remaining lifetime of an accepted JWT-SVID. It
/// matches the 300 second `max_assertion_lifetime` bound of the issuer profile.
pub const DEFAULT_JWT_SVID_MAX_LIFETIME: Duration = Duration::from_secs(300);
/// Largest ceiling a verifier may be configured with.
pub const MAX_JWT_SVID_LIFETIME_CEILING: Duration = Duration::from_secs(3600);
/// Default tolerated clock skew when checking `exp` and `nbf`.
pub const DEFAULT_JWT_SVID_LEEWAY: Duration = Duration::from_secs(30);
/// Largest clock skew a verifier may be configured with.
pub const MAX_JWT_SVID_LEEWAY: Duration = Duration::from_secs(60);
pub const MAX_JWT_SVID_BYTES: usize = 16 * 1024;
pub const MAX_JWT_AUDIENCE_BYTES: usize = 2048;
pub const JWT_SVID_USE: &str = "jwt-svid";
pub const MAX_JWKS_BYTES: usize = 256 * 1024;
pub const MAX_JWT_AUTHORITIES: usize = 16;
pub const MIN_RSA_MODULUS_BITS: usize = 2048;
/// Width of a P-521 coordinate, and of an ES512 signature (`R || S`, RFC 7518 3.4).
pub const P521_COORDINATE_BYTES: usize = 66;
pub const P521_SIGNATURE_BYTES: usize = 2 * P521_COORDINATE_BYTES;
/// How long the last delivered JWT bundle set stays trusted after the
/// Workload API stream is lost. A live stream is never stale: it delivers every
/// rotation and removal. The bound only limits how long a removed authority
/// can outlive a broken stream. It matches the default JWT-SVID clock leeway,
/// so it adds no window beyond what a token already tolerates, and it rides out
/// a reconnect or a brief agent restart. Past it the verifier fails closed.
pub const JWT_BUNDLE_MAX_STALENESS: Duration = Duration::from_secs(30);
pub const FETCH_X509_SVID: &str = "/SpiffeWorkloadAPI/FetchX509SVID";
pub const FETCH_JWT_BUNDLES: &str = "/SpiffeWorkloadAPI/FetchJWTBundles";
