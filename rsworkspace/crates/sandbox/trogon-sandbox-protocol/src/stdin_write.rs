use trogon_sandbox_contract::{StdinChunk, StdinEof};

/// One stdin write a worker applies to the process: its bytes and whether it closes stdin afterwards.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct StdinWrite {
    pub data: StdinChunk,
    pub eof: StdinEof,
}
