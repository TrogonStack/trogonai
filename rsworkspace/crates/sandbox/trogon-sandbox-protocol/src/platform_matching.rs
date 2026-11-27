use trogon_sandbox_contract::{MachServiceName, MandatoryDenySearchDepth};

use crate::MachServiceMatch;

/// How `name` is compared with a requested Mach service.
///
/// A trailing `*` makes the name a prefix, and anything else is an exact name.
pub fn mach_service_matcher(name: &MachServiceName) -> MachServiceMatch<'_> {
    match name.as_str().strip_suffix('*') {
        Some(prefix) => MachServiceMatch::Prefix(prefix),
        None => MachServiceMatch::Exact(name.as_str()),
    }
}

/// Reports whether a protected file lying `level` levels below the writable root is found by the scan.
pub fn search_depth_reaches_file(depth: MandatoryDenySearchDepth, level: u32) -> bool {
    level <= u32::from(depth.get())
}

/// Reports whether a protected directory lying `level` levels below the writable root is found by the scan.
///
/// It is one level shallower than a file.
pub fn search_depth_reaches_directory(depth: MandatoryDenySearchDepth, level: u32) -> bool {
    level < u32::from(depth.get())
}
