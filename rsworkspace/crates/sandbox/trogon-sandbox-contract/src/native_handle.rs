use serde::{Deserialize, Serialize};

use crate::ProviderKind;

/// The vendor-side identity of a lease's sandbox.
///
/// It carries no credentials: the caller authenticates to the vendor with their own.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeHandle {
    pub provider: ProviderKind,
    /// The vendor's own sandbox or container id.
    pub vendor_id: String,
    /// The vendor API resource for the sandbox, when the worker knows one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The region the sandbox was placed in, when the vendor reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}
