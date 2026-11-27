/// Assigns the next chunk's sequence number from a separate counter per
/// stream, so a consumer can detect gaps/reordering within stdout and within
/// stderr independently.
pub(super) struct ChunkSequencer {
    pub(super) stdout: u64,
    pub(super) stderr: u64,
}

impl ChunkSequencer {
    pub(super) fn new() -> Self {
        Self { stdout: 0, stderr: 0 }
    }

    pub(super) fn next(&mut self, stream: trogon_sandbox_contract::OutputStream) -> u64 {
        let counter = match stream {
            trogon_sandbox_contract::OutputStream::Stdout => &mut self.stdout,
            trogon_sandbox_contract::OutputStream::Stderr => &mut self.stderr,
        };
        let seq = *counter;
        *counter += 1;
        seq
    }
}
