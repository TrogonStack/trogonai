pub(super) const API_SOCKET: &str = "api.sock";
pub(super) const VSOCK_SOCKET: &str = "v.sock";
pub(super) const ROOTFS: &str = "rootfs.ext4";
pub(super) const LOG: &str = "firecracker.log";
pub(super) const PID_FILE: &str = "firecracker.pid";
/// `sun_path` is 104 bytes on macOS and 108 on Linux, and includes the terminating NUL.
pub(super) const MAX_SOCKET_PATH_BYTES: usize = 100;
/// Where released leases keep their snapshots; it is not a lease directory.
pub const RETAINED_DIRECTORY: &str = ".released";
