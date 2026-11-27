/// One ranged read of a sandbox file.
pub(crate) struct ReadChunk {
    pub(crate) data: Vec<u8>,
    pub(crate) total_size: u64,
    pub(crate) end: bool,
}
