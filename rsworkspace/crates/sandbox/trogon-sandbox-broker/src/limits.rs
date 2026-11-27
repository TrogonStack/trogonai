//! Per-tenant request limits.

mod in_memory_tenant_limits;
mod nats_kv_tenant_limits;
mod per_tag_quota;
mod tenant_limit;
mod tenant_limits;

pub use in_memory_tenant_limits::InMemoryTenantLimits;
pub use nats_kv_tenant_limits::NatsKvTenantLimits;
pub use per_tag_quota::PerTagQuota;
pub use tenant_limit::TenantLimit;
pub use tenant_limits::TenantLimits;

#[cfg(test)]
mod tests;
