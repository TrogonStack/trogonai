use trogon_sandbox_contract::{FileOp, FileOpErrorKind, FileOpResponse};

/// The stable lowercase token of an operation, for logs and error messages.
pub fn file_op_token(op: &FileOp) -> &'static str {
    match op {
        FileOp::List { .. } => "list",
        FileOp::Stat { .. } => "stat",
        FileOp::Read { .. } => "read",
        FileOp::Write { .. } => "write",
        FileOp::Edit { .. } => "edit",
        FileOp::Search { .. } => "search",
        FileOp::Remove { .. } => "remove",
        FileOp::Mkdir { .. } => "mkdir",
        FileOp::Rename { .. } => "rename",
        FileOp::ArchiveGet { .. } => "archive_get",
        FileOp::ArchivePut { .. } => "archive_put",
        FileOp::Checkpoint { .. } => "checkpoint",
        FileOp::Changes { .. } => "changes",
        FileOp::WatchStart { .. } => "watch_start",
        FileOp::WatchPoll { .. } => "watch_poll",
        FileOp::WatchStop { .. } => "watch_stop",
        FileOp::Checksum { .. } => "checksum",
    }
}

/// The reply of a provider that does not serve `op`, naming both.
pub fn unsupported_file_op_response(provider: &str, op: &FileOp) -> FileOpResponse {
    FileOpResponse::Unsupported {
        message: format!(
            "file operation {} is not supported by provider {provider}",
            file_op_token(op)
        ),
    }
}

/// Encodes a reply for a bus that carries at most `max_payload` bytes per message.
///
/// A reply that would not fit becomes a typed `too_large` failure, because a reply the bus refuses to publish
/// leaves the caller waiting for a timeout.
pub fn encode_file_op_response_within(response: &FileOpResponse, max_payload: usize) -> serde_json::Result<Vec<u8>> {
    let body = serde_json::to_vec(response)?;
    if body.len() <= max_payload {
        return Ok(body);
    }
    serde_json::to_vec(&FileOpResponse::Failed {
        kind: FileOpErrorKind::TooLarge,
        message: format!(
            "file operation reply is {} encoded bytes, over the {max_payload} byte transport limit",
            body.len()
        ),
    })
}
