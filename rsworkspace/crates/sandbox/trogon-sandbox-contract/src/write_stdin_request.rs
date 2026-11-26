use serde::{Deserialize, Serialize};

use crate::write_stdin_request_wire::WriteStdinRequestWire;
use crate::{ExecutionId, LeaseId, StdinChunk, StdinEof, StdinSeq, WriteStdinRequestError};

/// A client request to write bytes to, and optionally close, the stdin of a detached command.
///
/// The command must have started with [`StdinMode::Interactive`](crate::StdinMode::Interactive). `seq` makes
/// retries idempotent: the worker applies each `seq` once, in order. A write with no data and no end of input
/// carries nothing, so construction and deserialization reject it, and an oversized chunk is rejected as a
/// [`StdinChunk`] before it gets here.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "WriteStdinRequestWire", into = "WriteStdinRequestWire")]
pub struct WriteStdinRequest {
    lease_id: LeaseId,
    execution_id: ExecutionId,
    seq: StdinSeq,
    data: StdinChunk,
    eof: StdinEof,
}

impl WriteStdinRequest {
    pub fn new(
        lease_id: LeaseId,
        execution_id: ExecutionId,
        seq: StdinSeq,
        data: StdinChunk,
        eof: StdinEof,
    ) -> Result<Self, WriteStdinRequestError> {
        if data.as_slice().is_empty() && !eof.closes() {
            return Err(WriteStdinRequestError::Empty);
        }
        Ok(Self {
            lease_id,
            execution_id,
            seq,
            data,
            eof,
        })
    }

    pub fn lease_id(&self) -> &LeaseId {
        &self.lease_id
    }

    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }

    pub fn seq(&self) -> StdinSeq {
        self.seq
    }

    pub fn data(&self) -> &StdinChunk {
        &self.data
    }

    pub fn eof(&self) -> StdinEof {
        self.eof
    }
}

impl TryFrom<WriteStdinRequestWire> for WriteStdinRequest {
    type Error = WriteStdinRequestError;

    fn try_from(wire: WriteStdinRequestWire) -> Result<Self, Self::Error> {
        Self::new(wire.lease_id, wire.execution_id, wire.seq, wire.data, wire.eof)
    }
}

impl From<WriteStdinRequest> for WriteStdinRequestWire {
    fn from(request: WriteStdinRequest) -> Self {
        Self {
            lease_id: request.lease_id,
            execution_id: request.execution_id,
            seq: request.seq,
            data: request.data,
            eof: request.eof,
        }
    }
}
