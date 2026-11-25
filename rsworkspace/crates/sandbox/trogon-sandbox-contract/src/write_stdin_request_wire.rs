use serde::{Deserialize, Serialize};

use crate::{ExecutionId, LeaseId, StdinChunk, StdinEof, StdinSeq};

/// The JSON shape of a [`WriteStdinRequest`](crate::WriteStdinRequest) before its content is checked.
#[derive(Serialize, Deserialize)]
pub(crate) struct WriteStdinRequestWire {
    pub(crate) lease_id: LeaseId,
    pub(crate) execution_id: ExecutionId,
    pub(crate) seq: StdinSeq,
    #[serde(default)]
    pub(crate) data: StdinChunk,
    #[serde(default)]
    pub(crate) eof: StdinEof,
}
