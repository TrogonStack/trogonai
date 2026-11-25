use serde::Deserialize;

/// Unchecked substrate size class as it arrives on the wire.
#[derive(Deserialize)]
pub(crate) struct SubstrateSizeClassWire {
    pub(crate) label: String,
    pub(crate) memory_mib: u32,
}
