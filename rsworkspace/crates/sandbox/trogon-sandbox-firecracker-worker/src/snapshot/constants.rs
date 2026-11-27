pub const METADATA_FILE: &str = "snapshot.json";

pub const ROOTFS_FILE: &str = "rootfs.ext4";

/// Layer annotation naming which snapshot file a layer carries; the snapshot builder reads the same key.
pub const ROLE: &str = "io.trogon.sandbox.firecracker.file";

pub const MAX_TRANSFER_BYTES: u64 = 512 * 1024 * 1024;

pub const MAX_ROOTFS_BYTES: u64 = 4 * 1024 * 1024 * 1024;

pub const MAX_MEMORY_BYTES: u64 = 16 * 1024 * 1024 * 1024;

pub const MAX_METADATA_BYTES: u64 = 2 * 1024 * 1024;

pub const MAX_STATE_BYTES: u64 = 16 * 1024 * 1024;
