use serde::{Deserialize, Serialize};

use crate::{
    CapabilityRequirements, HostImport, IsolationTier, LeaseName, LeasePersistence, LeasePrewarm, LeaseRequestKey,
    OperationId, ProviderKind, SandboxSpec, SandboxTags, TenantId,
};

/// A tenant's request for a sandbox lease.
///
/// Optional fields are omitted from JSON when absent or empty and default when an older peer leaves them out.
/// `prewarm` and `persistent` are always written as JSON booleans.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct LeaseRequest {
    pub tenant: TenantId,
    pub provider_pref: Vec<ProviderKind>,
    pub spec: SandboxSpec,
    pub ttl_seconds: u32,
    pub idempotency_key: LeaseRequestKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation_tier: Option<IsolationTier>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_imports: Vec<HostImport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_capabilities: Option<CapabilityRequirements>,
    /// Marks a background tenant pool maintenance lease.
    #[serde(default)]
    pub prewarm: LeasePrewarm,
    /// Caller-defined key-value metadata.
    #[serde(default)]
    pub tags: SandboxTags,
    /// The stable name the lease can be reconnected by later. Without one the lease is anonymous and one-shot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<LeaseName>,
    /// Whether a named lease resumes from and captures to a snapshot tag derived from its name. Defaults to
    /// persistent.
    #[serde(default)]
    pub persistent: LeasePersistence,
    /// Vendor-specific options keyed by provider token, forwarded only to the worker of the provider that serves
    /// the lease. The contents are opaque here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<serde_json::Value>,
    /// The preferred region, in the vendor's own naming.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Regions to try, in order, when no worker can serve `region`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback_regions: Vec<String>,
    /// Opts the create into a durable operation the broker records and re-drives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
}
