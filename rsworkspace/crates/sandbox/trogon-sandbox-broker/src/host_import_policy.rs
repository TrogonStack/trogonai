//! Per-tenant host import policy.

#[expect(clippy::module_inception, reason = "the module holds the trait of the same name")]
mod host_import_policy;
mod in_memory_host_import_policy;
mod tenant_host_import_policy;

pub use host_import_policy::HostImportPolicy;
pub use in_memory_host_import_policy::InMemoryHostImportPolicy;
pub use tenant_host_import_policy::TenantHostImportPolicy;

#[cfg(test)]
mod tests;
