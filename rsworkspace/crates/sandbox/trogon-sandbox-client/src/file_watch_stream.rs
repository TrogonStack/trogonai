use std::collections::VecDeque;

use futures::stream::BoxStream;
use trogon_sandbox_contract::{FileOp, FileRecursion, LeaseId};

use crate::{ClientError, FileWatchUpdate, SandboxClient};

struct WatchState<'a> {
    client: &'a SandboxClient,
    lease_id: &'a LeaseId,
    path: &'a str,
    recursive: FileRecursion,
    watch_id: Option<String>,
    cursor: u64,
    pending: VecDeque<FileWatchUpdate>,
    done: bool,
}

impl SandboxClient {
    /// Watches `path` and yields its create/modify/delete events as they happen. The watch is started on first
    /// poll and stopped when the stream ends on an error; to stop one early, call [`Self::file_watch_stop`] (idle
    /// watches also expire at the worker). An error item ends the stream.
    pub fn watch_files<'a>(
        &'a self,
        lease_id: &'a LeaseId,
        path: &'a str,
        recursive: FileRecursion,
    ) -> BoxStream<'a, Result<FileWatchUpdate, ClientError>> {
        let state = WatchState {
            client: self,
            lease_id,
            path,
            recursive,
            watch_id: None,
            cursor: 0,
            pending: VecDeque::new(),
            done: false,
        };
        Box::pin(futures::stream::unfold(state, |mut state| async move {
            loop {
                if let Some(update) = state.pending.pop_front() {
                    return Some((Ok(update), state));
                }
                if state.done {
                    return None;
                }
                let watch_id = match &state.watch_id {
                    Some(id) => id.clone(),
                    None => match state
                        .client
                        .file_watch_start(state.lease_id, state.path, state.recursive)
                        .await
                    {
                        Ok(id) => {
                            state.watch_id = Some(id.clone());
                            id
                        }
                        Err(error) => {
                            state.done = true;
                            return Some((Err(error), state));
                        }
                    },
                };
                match state
                    .client
                    .file_watch_poll(state.lease_id, &watch_id, state.cursor, Some(FileOp::WATCH_MAX_WAIT_MS))
                    .await
                {
                    Ok((events, next_seq, overflowed)) => {
                        state.cursor = state.cursor.max(next_seq);
                        if overflowed.overflowed() {
                            state.pending.push_back(FileWatchUpdate::Overflowed);
                        }
                        state.pending.extend(events.into_iter().map(FileWatchUpdate::Event));
                    }
                    Err(error) => {
                        state.done = true;
                        let _ = state.client.file_watch_stop(state.lease_id, &watch_id).await;
                        return Some((Err(error), state));
                    }
                }
            }
        }))
    }
}
