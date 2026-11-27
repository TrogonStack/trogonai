use serde::Serialize;

/// The body a lease request gets back when it fails: the single field names the stage that refused.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LeaseErrorReply {
    /// The provider could not grant the lease.
    ProviderError(String),
    /// The dispatch payload could not be decoded or verified.
    DecodeError(String),
}

#[cfg(test)]
mod tests;
