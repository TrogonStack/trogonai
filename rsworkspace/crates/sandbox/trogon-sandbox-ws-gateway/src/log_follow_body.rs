use serde::Serialize;
use trogon_sandbox_client::{ClientError, LogFollowEvent};
use trogon_sandbox_contract::LogChunk;

use crate::client_error_code;

/// A log follow event as it travels on the wire. `chunk` carries the resume cursor; `end`, `unsupported` and
/// `error` finish the follow while `reconnecting` and `resync_required` are informational.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum LogFollowBody {
    Chunk(LogChunk),
    End { next_seq: u64, truncated: bool },
    Unsupported,
    Reconnecting { attempt: u32 },
    ResyncRequired { from_cursor: u64 },
    Error { code: &'static str, message: String },
}

impl From<&Result<LogFollowEvent, ClientError>> for LogFollowBody {
    fn from(item: &Result<LogFollowEvent, ClientError>) -> Self {
        match item {
            Ok(LogFollowEvent::Chunk(chunk)) => Self::Chunk(chunk.clone()),
            Ok(LogFollowEvent::End { next_seq, truncated }) => Self::End {
                next_seq: *next_seq,
                truncated: *truncated,
            },
            Ok(LogFollowEvent::Unsupported) => Self::Unsupported,
            Ok(LogFollowEvent::Reconnecting { attempt }) => Self::Reconnecting { attempt: *attempt },
            Ok(LogFollowEvent::ResyncRequired { from_cursor }) => Self::ResyncRequired {
                from_cursor: *from_cursor,
            },
            Err(error) => {
                let (code, message) = client_error_code(error);
                Self::Error { code, message }
            }
        }
    }
}

#[cfg(test)]
mod tests;
