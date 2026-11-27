use serde::{Deserialize, Serialize};

use crate::{LeaseError, NativeHandle};

/// The reply to a [`NativeHandleRequest`](crate::NativeHandleRequest), tagged by `outcome`.
///
/// `Unsupported` means the provider has no vendor handle to expose, or the owning worker does not declare
/// passthrough support.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum NativeHandleResponse {
    Available(NativeHandle),
    Unsupported,
    Denied(LeaseError),
}
