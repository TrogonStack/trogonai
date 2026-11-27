//! Durable, pageable command output.
//!
//! Workers publish each streamed [`CommandOutputChunk`] to
//! `sandbox.evidence.{lease}.command.{execution}`. That subject space is plain
//! core NATS until a JetStream stream captures it, so a client that reloads, or
//! a second broker node, would have no way to read output it missed. The
//! broker owns one stream over those subjects ([`JetStreamCommandLogStore`])
//! and answers `ReadLogsRequest` pages from it by direct get, using the
//! stream sequence as the cursor.
//!
//! Retention is bounded by [`CommandLogRetention`]: the stream keeps at most
//! `max_bytes` in total and `max_age` per chunk, and drops the oldest first.
//! Chunks are stored exactly as the worker published them, after the worker's
//! own redaction.
//!
//! The stream also keeps each execution's [`CommandExitMarker`] from
//! `sandbox.evidence.{lease}.command_exit.{execution}`, so a page can say the
//! command has exited and a parked follower wakes when it does instead of
//! asking the worker once a second.
//!
//! Exit status outlives output. A second stream ([`COMMAND_EXIT_STREAM`])
//! sources only the exit markers from the log stream and keeps them for
//! [`CommandLogRetention::exit_max_age`], which is longer than `max_age`. A
//! reader that arrives after the output aged out still learns the command
//! exited: [`CommandLogStore::read`] falls back to that record.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

use async_nats::jetstream::{self, consumer, stream};
use async_trait::async_trait;
use trogon_sandbox_contract::{CommandExitMarker, CommandOutputChunk, ExecutionId, LeaseId, LogChunk};
use trogon_sandbox_protocol::subjects::{command_exit_subject, evidence_command_suffix};

mod command_log_store;
mod in_memory_command_log_store;
mod jet_stream_command_log_store;

pub use command_log_store::CommandLogStore;
pub use in_memory_command_log_store::InMemoryCommandLogStore;
pub use jet_stream_command_log_store::JetStreamCommandLogStore;

pub use crate::command_log_error::CommandLogError;
pub use crate::command_log_retention::CommandLogRetention;
use crate::constants::LATE_CHUNK_SWEEP;
pub use crate::constants::{
    COMMAND_EXIT_STREAM, COMMAND_EXIT_SUBJECT_FILTER, COMMAND_LOG_STREAM, COMMAND_LOG_SUBJECT_FILTER,
    MAX_CHUNKS_PER_PAGE,
};
pub use crate::log_page::LogPage;

/// Whether `execution_id` is safe to place in a subject token.
pub fn valid_execution_id(execution_id: &ExecutionId) -> bool {
    let value = execution_id.as_str();
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

async fn purge_filters(stream: &stream::Stream, filters: &[String]) -> Result<(), CommandLogError> {
    for filter in filters {
        stream
            .purge()
            .filter(filter.clone())
            .await
            .map_err(CommandLogError::backend)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
