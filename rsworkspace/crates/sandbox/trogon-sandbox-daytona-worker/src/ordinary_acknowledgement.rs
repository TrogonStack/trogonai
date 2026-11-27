use serde::Deserialize;

use crate::ordinary_protocol::VERSION;

/// The bridge's answer to one control frame.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OrdinaryAcknowledgement {
    pub(crate) version: u32,
    pub(crate) execution_id: String,
    pub(crate) nonce: String,
    pub(crate) request_id: u64,
    pub(crate) operation: String,
    pub(crate) signal: Option<u32>,
    pub(crate) delivered: Option<bool>,
    pub(crate) bytes_written: usize,
    pub(crate) eof: bool,
}

impl OrdinaryAcknowledgement {
    /// Whether the answer belongs to this execution, speaks this protocol and does not answer a request that was
    /// never sent.
    pub(crate) fn is_bound(&self, execution: &str, nonce: &str, sequence: u64) -> bool {
        self.version == VERSION && self.execution_id == execution && self.nonce == nonce && self.request_id <= sequence
    }

    /// Whether the answer reports no delivery, bytes or end of input, as `launch`, `init` and `status` must.
    pub(crate) fn is_plain(&self) -> bool {
        self.delivered.is_none() && self.bytes_written == 0 && !self.eof
    }
}

#[cfg(test)]
mod tests;
