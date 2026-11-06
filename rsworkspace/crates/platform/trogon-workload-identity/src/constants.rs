use std::time::Duration;

pub const MAX_REGISTRATION_LIFETIME: Duration = Duration::from_secs(330);
pub const MAX_SVID_LIFETIME: Duration = Duration::from_secs(3600);
pub const MAX_BINDINGS: usize = 256;
pub const MAX_ROOTS: usize = 16;
pub const MAX_IDENTITIES: usize = 32;
pub const MAX_CONTEXT_BYTES: usize = 1024 * 1024;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const RECONNECT_DELAY: Duration = Duration::from_millis(250);
pub const LIFECYCLE_POLL: Duration = Duration::from_millis(100);
pub const TEST_TIMEOUT: Duration = Duration::from_secs(5);
