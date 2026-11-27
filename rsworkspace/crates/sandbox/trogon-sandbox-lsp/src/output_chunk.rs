use crate::output_kind::OutputKind;

/// One piece of the language server's output. `seq` counts per [`OutputKind`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct OutputChunk {
    pub kind: OutputKind,
    pub seq: u64,
    pub data: Vec<u8>,
}
