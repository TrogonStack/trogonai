use crate::MaterialSuite;

#[derive(Clone, PartialEq, Eq)]
pub struct WrappedDek {
    suite: MaterialSuite,
    format_version: u32,
    adapter_payload: Vec<u8>,
}

impl WrappedDek {
    pub fn new(suite: MaterialSuite, format_version: u32, adapter_payload: Vec<u8>) -> Result<Self, WrappedDekError> {
        if format_version != 1 || adapter_payload.is_empty() || adapter_payload.len() > 65_536 {
            return Err(WrappedDekError);
        }
        Ok(Self {
            suite,
            format_version,
            adapter_payload,
        })
    }
    pub fn suite(&self) -> MaterialSuite {
        self.suite
    }
    pub fn format_version(&self) -> u32 {
        self.format_version
    }
    pub fn adapter_payload(&self) -> &[u8] {
        &self.adapter_payload
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid wrapped DEK version or payload length")]
pub struct WrappedDekError;

impl std::fmt::Debug for WrappedDek {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WrappedDek")
            .field("suite", &self.suite)
            .field("format_version", &self.format_version)
            .field("payload_len", &self.adapter_payload.len())
            .finish()
    }
}
