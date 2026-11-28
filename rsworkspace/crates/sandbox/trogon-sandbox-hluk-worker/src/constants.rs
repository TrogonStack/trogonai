/// Where a lease's workspace appears in the guest, and the directory a command runs in when it names none.
pub const WORKSPACE_GUEST_DIR: &str = "/workspace";

/// Where a lease's scratch directory appears in the guest, over the image's own `/tmp`.
pub const TMP_GUEST_DIR: &str = "/tmp";

/// Queue group the workers share, so one of them takes each lease dispatch.
pub const LEASE_QUEUE_GROUP: &str = "sandbox-provider-hluk";

/// Exit code reported when `hluk` ends without one, as a signal would.
pub const KILLED_EXIT_CODE: i32 = 137;

/// Why a request that names another user is refused.
pub const ONE_USER: &str = "hluk guests have one user; acting as another user is not supported";
