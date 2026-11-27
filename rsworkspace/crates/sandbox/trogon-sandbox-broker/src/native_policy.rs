//! Tenant policy for the vendor passthrough escape hatch (`LeaseRequest::native`
//! and the `native` handle verb). Passthrough bypasses the portable contract,
//! so the default is deny and a tenant has to be allowed explicitly.
//!
//! Operators allow a tenant either in the broker's static list (CLI or env) or
//! with a per-tenant record in the NATS KV store that the admin tool
//! native-passthrough command writes. A tenant on the static list is always
//! allowed;
//! any other tenant is allowed only while its stored record says so.

mod deny_native_passthrough;
mod in_memory_native_passthrough_store;
mod native_passthrough_policy;
mod nats_kv_native_passthrough_store;
mod static_then_stored_native_passthrough;
mod tenant_allow_list_native_passthrough;
mod tenant_native_passthrough_policy;

pub use deny_native_passthrough::DenyNativePassthrough;
pub use in_memory_native_passthrough_store::InMemoryNativePassthroughStore;
pub use native_passthrough_policy::NativePassthroughPolicy;
pub use nats_kv_native_passthrough_store::NatsKvNativePassthroughStore;
pub use static_then_stored_native_passthrough::StaticThenStoredNativePassthrough;
pub use tenant_allow_list_native_passthrough::TenantAllowListNativePassthrough;
pub use tenant_native_passthrough_policy::TenantNativePassthroughPolicy;

#[cfg(test)]
mod tests;
