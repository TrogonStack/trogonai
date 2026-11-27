use std::ffi::OsString;
use std::path::Path;

use trogon_sandbox_contract::{HostMountAccess, RemoteMountSource};

use crate::{FuseMountConfig, FuseOtherUsers};

/// The `mount-s3` command line that mounts `source` at `host_dir`.
pub fn build_mount_args(
    config: &FuseMountConfig,
    source: &RemoteMountSource,
    host_dir: &Path,
    access: HostMountAccess,
) -> Vec<OsString> {
    let RemoteMountSource::S3 { bucket, prefix } = source;
    let mut args: Vec<OsString> = vec![bucket.as_str().into(), host_dir.into()];

    if !prefix.is_empty() {
        args.push("--prefix".into());
        args.push(if prefix.ends_with('/') {
            prefix.into()
        } else {
            format!("{prefix}/").into()
        });
    }
    if let Some(region) = &config.region {
        args.push("--region".into());
        args.push(region.as_str().into());
    }
    if let Some(endpoint) = &config.endpoint {
        args.push("--endpoint-url".into());
        args.push(endpoint.as_str().into());
    }
    if access.is_read_only() {
        args.push("--read-only".into());
    }
    if config.other_users == FuseOtherUsers::Allowed {
        args.push("--allow-other".into());
    }
    args
}
