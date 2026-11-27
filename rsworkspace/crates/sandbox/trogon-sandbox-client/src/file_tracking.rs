use trogon_sandbox_contract::{
    FileContentHashing, FileOp, FileOpResponse, FilePresence, FileRecursion, FileWatchEvent, FileWatchOverflow, LeaseId,
};

use crate::{ClientError, FileChanges, SandboxClient};

impl SandboxClient {
    /// Records the state of the tree under `path` and returns the checkpoint id with the number of entries seen.
    pub async fn file_checkpoint(
        &self,
        lease_id: &LeaseId,
        path: &str,
        hash: FileContentHashing,
    ) -> Result<(String, u64), ClientError> {
        let op = FileOp::Checkpoint {
            path: path.to_string(),
            hash,
        };
        match self.file_op(lease_id, op, None).await? {
            FileOpResponse::Checkpointed { checkpoint_id, entries } => Ok((checkpoint_id, entries)),
            other => Err(Self::unexpected_file_reply("checkpoint", other)),
        }
    }

    /// Paths added, modified and deleted since `since` (a checkpoint id, or the lease start when `None`),
    /// optionally narrowed to the subtree `path`.
    pub async fn file_changes(
        &self,
        lease_id: &LeaseId,
        since: Option<String>,
        path: Option<String>,
    ) -> Result<FileChanges, ClientError> {
        let op = FileOp::Changes { since, path };
        match self.file_op(lease_id, op, None).await? {
            FileOpResponse::Changes {
                since,
                added,
                modified,
                deleted,
                truncated,
            } => Ok(FileChanges {
                since,
                added,
                modified,
                deleted,
                truncated,
            }),
            other => Err(Self::unexpected_file_reply("changes", other)),
        }
    }

    /// Starts watching `path` and returns the watch id for [`Self::file_watch_poll`].
    pub async fn file_watch_start(
        &self,
        lease_id: &LeaseId,
        path: &str,
        recursive: FileRecursion,
    ) -> Result<String, ClientError> {
        let op = FileOp::WatchStart {
            path: path.to_string(),
            recursive,
        };
        match self.file_op(lease_id, op, None).await? {
            FileOpResponse::WatchStarted { watch_id } => Ok(watch_id),
            other => Err(Self::unexpected_file_reply("watch_start", other)),
        }
    }

    /// Events of a watch after `after_seq`, parking at the worker up to `wait_ms` when there are none yet. Returns
    /// `(events, next_seq, overflowed)`.
    pub async fn file_watch_poll(
        &self,
        lease_id: &LeaseId,
        watch_id: &str,
        after_seq: u64,
        wait_ms: Option<u32>,
    ) -> Result<(Vec<FileWatchEvent>, u64, FileWatchOverflow), ClientError> {
        let op = FileOp::WatchPoll {
            watch_id: watch_id.to_string(),
            after_seq,
            wait_ms,
        };
        match self.file_op(lease_id, op, None).await? {
            FileOpResponse::WatchEvents {
                events,
                next_seq,
                overflowed,
            } => Ok((events, next_seq, overflowed)),
            other => Err(Self::unexpected_file_reply("watch_poll", other)),
        }
    }

    /// Stops a watch. Returns whether it existed, so stopping twice is harmless.
    pub async fn file_watch_stop(&self, lease_id: &LeaseId, watch_id: &str) -> Result<FilePresence, ClientError> {
        let op = FileOp::WatchStop {
            watch_id: watch_id.to_string(),
        };
        match self.file_op(lease_id, op, None).await? {
            FileOpResponse::WatchStopped { existed } => Ok(existed),
            other => Err(Self::unexpected_file_reply("watch_stop", other)),
        }
    }
}
