pub const METADATA_FILE: &str = "snapshot.json";

pub const ROOTFS_FILE: &str = "rootfs.ext4";

pub const ROLE: &str = "io.sandbox.firecracker.file";

pub const MAX_TRANSFER_BYTES: u64 = 512 * 1024 * 1024;

pub const MAX_ROOTFS_BYTES: u64 = 4 * 1024 * 1024 * 1024;

pub const MAX_MEMORY_BYTES: u64 = 16 * 1024 * 1024 * 1024;

pub const MAX_METADATA_BYTES: u64 = 2 * 1024 * 1024;

pub const MAX_STATE_BYTES: u64 = 16 * 1024 * 1024;
