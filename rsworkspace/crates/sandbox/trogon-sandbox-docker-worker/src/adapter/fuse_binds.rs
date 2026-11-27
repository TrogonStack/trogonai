use super::*;

pub fn build_fuse_bind_strings(mounts_with_dirs: &[(RemoteMount, PathBuf)]) -> Vec<String> {
    mounts_with_dirs
        .iter()
        .map(|(mount, host_dir)| {
            let host = host_dir.to_string_lossy();
            if mount.access.is_read_only() {
                format!("{host}:{}:ro", mount.mount_path)
            } else {
                format!("{host}:{}", mount.mount_path)
            }
        })
        .collect()
}
