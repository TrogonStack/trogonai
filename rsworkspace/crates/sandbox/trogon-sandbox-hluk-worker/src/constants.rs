/// Where a lease's workspace appears in the guest, and the directory a command runs in when it names none.
pub const WORKSPACE_GUEST_DIR: &str = "/workspace";

/// Queue group the workers share, so one of them takes each lease dispatch.
pub const LEASE_QUEUE_GROUP: &str = "sandbox-provider-hluk";

/// Exit code reported when `hluk` ends without one, as a signal would.
pub const KILLED_EXIT_CODE: i32 = 137;
