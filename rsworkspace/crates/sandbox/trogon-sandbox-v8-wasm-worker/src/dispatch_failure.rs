use serde::Serialize;

/// The reply to a lease dispatch that did not produce a grant.
#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum DispatchFailure {
    /// The dispatch payload could not be decoded.
    Decode { decode_error: String },
    /// The provider refused or failed the lease.
    Provider { provider_error: String },
}
