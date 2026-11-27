use tokio::sync::Mutex;
use tokio::time::{Instant, sleep};
use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{StdinEof, WriteStdinResponse};

use crate::backpressure_policy::BackpressurePolicy;
use crate::constants::{MAX_STALLED_RESENDS, MAX_TRANSPORT_RETRIES, TRANSPORT_RETRY_DELAY};
use crate::lsp_error::LspError;
use crate::stdin_sink::StdinSink;
use crate::stdin_state::{PendingChunk, StdinState};

/// Serializes stdin writes, numbers them, and retries each chunk under the same `seq` when the
/// worker reports backpressure or the transport fails, so delivery is ordered and at-most-once.
pub struct StdinWriter<S> {
    sink: S,
    policy: BackpressurePolicy,
    state: Mutex<StdinState>,
}

impl<S: StdinSink> StdinWriter<S> {
    pub fn new(sink: S) -> Self {
        Self::with_policy(sink, BackpressurePolicy::default())
    }

    pub fn with_policy(sink: S, policy: BackpressurePolicy) -> Self {
        Self {
            sink,
            policy,
            state: Mutex::new(StdinState::default()),
        }
    }

    pub async fn write(&self, data: &[u8], eof: StdinEof) -> Result<(), LspError> {
        let mut state = self.state.lock().await;
        if let Some(failure) = &state.failure {
            return Err(failure.clone());
        }
        if state.eof_queued {
            return Err(LspError::Stdin("stdin is closed".into()));
        }
        state.queue_chunks(data, eof)?;
        self.flush(&mut state).await
    }

    /// Sends EOF on stdin. Safe to call more than once.
    pub async fn close(&self) -> Result<(), LspError> {
        let mut state = self.state.lock().await;
        if let Some(failure) = &state.failure {
            return Err(failure.clone());
        }
        if !state.eof_queued {
            state.queue_chunks(&[], StdinEof::Close)?;
        }
        self.flush(&mut state).await
    }

    async fn flush(&self, state: &mut StdinState) -> Result<(), LspError> {
        let mut stalled = 0;
        while let Some(chunk) = state.unacked.front().cloned() {
            let response = self.send_chunk(&chunk).await?;
            if response == WriteStdinResponse::AlreadyCompleted && chunk.eof.closes() {
                state.unacked.clear();
                return Ok(());
            }
            let (advanced, next_seq) = match response {
                WriteStdinResponse::Written { next_seq } | WriteStdinResponse::Duplicate { next_seq } => {
                    (true, next_seq)
                }
                WriteStdinResponse::Buffered { next_seq } | WriteStdinResponse::OutOfWindow { next_seq } => {
                    (false, next_seq)
                }
                other => {
                    let failure = LspError::Stdin(format!("stdin write failed: {other:?}"));
                    state.failure = Some(failure.clone());
                    state.unacked.clear();
                    return Err(failure);
                }
            };
            if advanced || next_seq > chunk.seq {
                state.unacked.retain(|c| c.seq > chunk.seq && c.seq >= next_seq);
                stalled = 0;
                continue;
            }
            if next_seq < chunk.seq {
                return Err(LspError::Stdin(format!(
                    "server expects stdin seq {} which was already acknowledged",
                    next_seq.get()
                )));
            }
            stalled += 1;
            if stalled > MAX_STALLED_RESENDS {
                return Err(LspError::Stdin(format!(
                    "stdin made no progress at seq {}",
                    next_seq.get()
                )));
            }
        }
        Ok(())
    }

    async fn send_chunk(&self, chunk: &PendingChunk) -> Result<WriteStdinResponse, LspError> {
        let mut transport_attempt = 0;
        let mut backoff = self.policy.initial_delay;
        let started = Instant::now();
        loop {
            let response = match self.sink.send(chunk.seq, chunk.data.clone(), chunk.eof).await {
                Ok(response) => response,
                Err(err) => {
                    let retryable = matches!(err, ClientError::Transport(_) | ClientError::Timeout(_));
                    if !retryable || transport_attempt >= MAX_TRANSPORT_RETRIES {
                        return Err(LspError::from(err));
                    }
                    transport_attempt += 1;
                    sleep(TRANSPORT_RETRY_DELAY * transport_attempt).await;
                    continue;
                }
            };
            if !matches!(response, WriteStdinResponse::Backpressure { .. }) {
                return Ok(response);
            }
            let elapsed = started.elapsed();
            if elapsed >= self.policy.budget {
                return Err(LspError::Stdin(format!(
                    "stdin stayed backpressured at seq {} for {}ms",
                    chunk.seq.get(),
                    elapsed.as_millis()
                )));
            }
            sleep(backoff).await;
            backoff = (backoff * 2).min(self.policy.max_delay);
        }
    }
}
