use trogon_sandbox_contract::LogChunk;

/// One item of [`SandboxClient::follow_logs`](crate::SandboxClient::follow_logs).
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum LogFollowEvent {
    /// Output, in order. `log_seq` is the resume cursor.
    Chunk(LogChunk),
    /// The command exited and all its retained output was delivered. `next_seq` is the final cursor; `truncated`
    /// is set when retention had already dropped the start of the output.
    End { next_seq: u64, truncated: bool },
    /// The broker keeps no command logs, so there is nothing to follow.
    Unsupported,
    /// A read failed with a timeout or transport error and the follower is retrying from its cursor after a short
    /// backoff. `attempt` counts consecutive failures, starting at 1; no output is lost or repeated.
    Reconnecting { attempt: u32 },
    /// Retention dropped output between `from_cursor` (the last cursor the follower delivered, `0` when it started
    /// from the beginning) and the next chunk. Treat what follows as a stream with a hole in it; re-read the
    /// command's result if the missing output matters.
    ResyncRequired { from_cursor: u64 },
}
