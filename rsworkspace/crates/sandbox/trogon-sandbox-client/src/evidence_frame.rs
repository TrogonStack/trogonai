use bytes::Bytes;

/// One message published under a lease's evidence subject.
///
/// A caller wanting a specific command's streamed output (from a command run with streamed output) should filter
/// `EvidenceFrame::subject_suffix` against `trogon_sandbox_protocol::subjects::evidence_command_suffix(execution_id)`
/// and deserialize `EvidenceFrame::payload` as a `CommandOutputChunk` themselves.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct EvidenceFrame {
    pub subject_suffix: String,
    pub payload: Bytes,
}
