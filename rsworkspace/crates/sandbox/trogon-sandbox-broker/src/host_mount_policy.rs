//! Per-tenant host mount allow-lists.

mod host_mount_allow_list;
mod in_memory_host_mount_allow_list;
mod nats_kv_host_mount_allow_list;
mod tenant_host_mount_policy;

pub use host_mount_allow_list::HostMountAllowList;
pub use in_memory_host_mount_allow_list::InMemoryHostMountAllowList;
pub use nats_kv_host_mount_allow_list::NatsKvHostMountAllowList;
pub use tenant_host_mount_policy::TenantHostMountPolicy;

pub fn path_prefix_matches(prefix: &str, path: &str) -> bool {
    if prefix.is_empty() || path.is_empty() {
        return false;
    }
    if !prefix.starts_with('/') {
        return false;
    }
    path == prefix || path.starts_with(&format!("{prefix}/"))
}

#[cfg(test)]
mod tests;
