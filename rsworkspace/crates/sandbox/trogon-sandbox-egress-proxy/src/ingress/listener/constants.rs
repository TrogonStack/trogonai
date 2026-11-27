pub(super) const UNIX_PREFIX: &str = "unix:";

#[cfg(unix)]
pub(super) const UNIX_SOCKET_MODE: u32 = 0o600;
