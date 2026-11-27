/// The queue group the workers of this provider share, so one worker takes each lease dispatch.
pub(super) const LEASE_QUEUE: &str = "sandbox-provider-subprocess";

/// Denial reason shared by the create-user, create-group and modify-group-membership handlers.
///
/// This provider isolates each lease with bubblewrap's `--unshare-all`, which creates a
/// per-process Linux user namespace rather than real host-visible identities, and the worker
/// container itself runs as an unprivileged user, so it has no privilege to write
/// `/etc/passwd` or `/etc/group`. `useradd`, `groupadd`, `usermod` and `gpasswd` run inside
/// that sandbox would either fail outright or mutate an ephemeral namespace-local view that
/// disappears when the process exits, never a real system user. An owned lease therefore gets
/// a fast, honest `Denied` reply instead of a silent no-op or a timeout.
pub(super) const SANDBOX_USERS_UNSUPPORTED_MESSAGE: &str = "the subprocess/bubblewrap provider isolates each lease in its own Linux user namespace (--unshare-all); useradd/groupadd cannot create real, host-visible system users inside that namespace, only ephemeral namespace-local state that vanishes with the process; real per-agent Linux users are not supported on this provider";
