use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use futures::stream::BoxStream;
use trogon_sandbox_contract::{
    ExecutionId, LeaseId, OutputStream, PollCommandRequest, PollCommandResponse, ReadLogsRequest, ReadLogsResponse,
};
use trogon_sandbox_protocol::subjects;

use crate::constants::{FOLLOW_LOGS_RECONNECT_BACKOFF, FOLLOW_LOGS_RECONNECT_BACKOFF_MAX, FOLLOW_LOGS_WAIT};
use crate::{ClientError, FOLLOW_LOGS_MAX_RECONNECTS, LogFollowEvent, SandboxClient};

struct FollowState<'a> {
    client: &'a SandboxClient,
    lease_id: &'a LeaseId,
    execution_id: ExecutionId,
    cursor: u64,
    pending: VecDeque<LogFollowEvent>,
    /// Cursor the follow started from; `0` means the caller asked for the whole output, so a dropped start is a
    /// loss.
    started_at: u64,
    /// Highest per-stream `seq` delivered, to spot dropped chunks.
    last_seq: HashMap<OutputStream, u64>,
    truncated: bool,
    exited: bool,
    done: bool,
    reconnects: u32,
}

impl FollowState<'_> {
    /// Counts one more failed attempt, waits out its backoff, and returns the event announcing the retry.
    async fn reconnect(&mut self) -> LogFollowEvent {
        self.reconnects += 1;
        let attempt = self.reconnects;
        let backoff = FOLLOW_LOGS_RECONNECT_BACKOFF
            .saturating_mul(1 << (attempt - 1).min(5))
            .min(FOLLOW_LOGS_RECONNECT_BACKOFF_MAX);
        tokio::time::sleep(backoff).await;
        LogFollowEvent::Reconnecting { attempt }
    }

    async fn read(&self, wait: Option<Duration>) -> Result<ReadLogsResponse, ClientError> {
        let request = ReadLogsRequest {
            lease_id: self.lease_id.clone(),
            execution_id: self.execution_id.clone(),
            after_seq: self.cursor,
            max_bytes: None,
            wait_ms: wait.map(|wait| u64::try_from(wait.as_millis()).unwrap_or(u64::MAX)),
        };
        let timeout = self.client.default_timeout + wait.unwrap_or_default();
        self.client
            .send(&subjects::lease_read_logs(self.lease_id), &request, timeout)
            .await
    }

    async fn poll(&self) -> Result<PollCommandResponse, ClientError> {
        let request = PollCommandRequest {
            lease_id: self.lease_id.clone(),
            execution_id: self.execution_id.clone(),
        };
        self.client
            .send_default(&subjects::lease_poll_command(self.lease_id), &request)
            .await
    }

    /// Advances the follow until it has an item to yield. `None` ends the stream.
    async fn next_item(&mut self) -> Option<Result<LogFollowEvent, ClientError>> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Some(Ok(event));
            }
            if self.done {
                return None;
            }
            // Once the command is known to have exited, only drain: a wait would just run out the clock.
            let wait = if self.exited { None } else { Some(FOLLOW_LOGS_WAIT) };
            let page = self.read(wait).await;
            if page.is_ok() {
                self.reconnects = 0;
            }
            match page {
                // The only failures a read can report are a missing reply and a bus or decode failure. Reads are
                // idempotent, so retrying from the cursor cannot repeat output.
                Err(error) => {
                    if self.reconnects >= FOLLOW_LOGS_MAX_RECONNECTS {
                        self.done = true;
                        return Some(Err(error));
                    }
                    return Some(Ok(self.reconnect().await));
                }
                Ok(ReadLogsResponse::Unsupported) => {
                    self.done = true;
                    return Some(Ok(LogFollowEvent::Unsupported));
                }
                Ok(ReadLogsResponse::Denied(error)) => {
                    self.done = true;
                    return Some(Err(ClientError::Denied(error)));
                }
                Ok(ReadLogsResponse::Logs {
                    chunks,
                    next_seq,
                    has_more,
                    truncated,
                    exited,
                }) => {
                    let truncated = truncated.is_truncated();
                    // Retention dropped the start of output the caller asked to read in full.
                    if truncated && !self.truncated && self.started_at == 0 {
                        self.pending
                            .push_back(LogFollowEvent::ResyncRequired { from_cursor: 0 });
                    }
                    self.truncated |= truncated;
                    // The worker's exit marker follows its last chunk, so once the broker reports it, catching up
                    // is the end.
                    self.exited |= exited.is_reported();
                    let mut delivered = self.cursor;
                    self.cursor = self.cursor.max(next_seq);
                    let caught_up = chunks.is_empty() && !has_more.has_more();
                    for chunk in chunks {
                        let gap = self
                            .last_seq
                            .get(&chunk.stream)
                            .is_some_and(|last| chunk.seq > last.saturating_add(1));
                        if gap {
                            self.pending
                                .push_back(LogFollowEvent::ResyncRequired { from_cursor: delivered });
                        }
                        self.last_seq.insert(chunk.stream, chunk.seq);
                        delivered = chunk.log_seq;
                        self.pending.push_back(LogFollowEvent::Chunk(chunk));
                    }
                    if !caught_up || !self.pending.is_empty() {
                        continue;
                    }
                    if self.exited {
                        self.done = true;
                        return Some(Ok(LogFollowEvent::End {
                            next_seq: self.cursor,
                            truncated: self.truncated,
                        }));
                    }
                    // Caught up and the wait ran out: has it exited?
                    match self.poll().await {
                        Ok(PollCommandResponse::Running) => {}
                        Ok(PollCommandResponse::Completed(_)) | Ok(PollCommandResponse::NotFound) => self.exited = true,
                        Ok(PollCommandResponse::Denied(error)) => {
                            self.done = true;
                            return Some(Err(ClientError::Denied(error)));
                        }
                        // The read above succeeded and reset the count, so a failed poll always has a retry left.
                        Err(_) => return Some(Ok(self.reconnect().await)),
                    }
                }
            }
        }
    }
}

impl SandboxClient {
    /// Follows a streamed command's output from the `after_seq` cursor: the retained chunks first, then new ones as
    /// they are produced, each with its `log_seq`. The stream ends with [`LogFollowEvent::End`] once the command
    /// has exited and everything it wrote was delivered.
    ///
    /// The cursor is the broker's, so a caller that lost its connection restarts with the last `log_seq` it saw and
    /// gets neither a gap nor a repeat. A broker timeout or transport failure is retried in place, up to
    /// [`FOLLOW_LOGS_MAX_RECONNECTS`] times in a row, announcing each retry with [`LogFollowEvent::Reconnecting`].
    /// When retention dropped output the follower had not read yet, [`LogFollowEvent::ResyncRequired`] precedes the
    /// first chunk after the gap, so the loss is never silent. Any other error item, or one after the retries ran
    /// out, ends the stream; resume from the last cursor.
    pub fn follow_logs<'a>(
        &'a self,
        lease_id: &'a LeaseId,
        execution_id: ExecutionId,
        after_seq: u64,
    ) -> BoxStream<'a, Result<LogFollowEvent, ClientError>> {
        let state = FollowState {
            client: self,
            lease_id,
            execution_id,
            cursor: after_seq,
            pending: VecDeque::new(),
            started_at: after_seq,
            last_seq: HashMap::new(),
            truncated: false,
            exited: false,
            done: false,
            reconnects: 0,
        };
        Box::pin(futures::stream::unfold(state, |mut state| async move {
            let item = state.next_item().await?;
            Some((item, state))
        }))
    }
}
