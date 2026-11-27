use std::path::Path;

/// Whether a mount-table listing has a FUSE filesystem mounted at `host_dir`.
pub fn parse_proc_mounts_for_fuse(contents: &str, host_dir: &Path) -> bool {
    let dir = host_dir.to_string_lossy();
    contents.lines().any(|line| {
        let mut parts = line.split_whitespace();
        let _device = parts.next();
        let mount_point = parts.next().unwrap_or_default();
        let fstype = parts.next().unwrap_or_default();
        mount_point == dir && fstype.starts_with("fuse")
    })
}
