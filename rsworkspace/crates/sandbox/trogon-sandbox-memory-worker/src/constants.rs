/// The longest a `sleep` command waits, so a huge argument cannot overflow a duration.
pub(crate) const MAX_SLEEP_SECONDS: f64 = 86_400.0;

/// Directories that exist before anything is written.
pub(crate) const STANDARD_DIRS: &[&str] = &["/home", "/root", "/tmp", "/workspace"];

/// Where a command runs when its request names no cwd.
pub(crate) const DEFAULT_CWD: &str = "/";

/// The exit code a SIGKILLed command reports.
pub(crate) const KILLED_EXIT_CODE: i32 = 137;

/// Key under which a lease's tracking state is held. Each lease owns its own `Tracking`, so
/// one lease can never name another's checkpoint or watch.
pub(crate) const TRACKING_KEY: &str = "lease";
