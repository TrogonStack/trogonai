//! A fake broker that serves a growing command log, for the `follow_logs` tests.

#![allow(dead_code)]

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use futures::stream::BoxStream;
use trogon_sandbox_client::{Bus, BusError, BusMessage, ClientError, LogFollowEvent, SandboxClient};
use trogon_sandbox_contract::{
    CommandExitReport, CommandOutput, ExecutionId, LeaseId, LogChunk, LogsContinuation, OutputStream, OutputTruncation,
    PollCommandResponse, ReadLogsRequest, ReadLogsResponse,
};

/// Serves `chunks` as a growing log: each read that finds nothing new reveals one more chunk, as a long poll would
/// see one land. A command counts as exited once every chunk is visible.
pub struct FakeLogBroker {
    pub chunks: Vec<LogChunk>,
    visible: Mutex<usize>,
    page_chunks: usize,
    longest_wait_ms: Mutex<u64>,
    /// Whether `read_logs` reports the worker's exit marker.
    marks_exit: bool,
    polls: Mutex<u32>,
    /// Reads that fail with a bus error before the broker answers.
    failing_reads: Mutex<u32>,
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, BusError> {
    mutex
        .lock()
        .map_err(|_| BusError::Custom("fake broker state poisoned".into()))
}

fn custom(error: serde_json::Error) -> BusError {
    BusError::Custom(Box::new(error))
}

impl FakeLogBroker {
    pub fn new(log_seqs: &[u64], visible: usize, page_chunks: usize) -> Self {
        Self {
            chunks: log_seqs
                .iter()
                .enumerate()
                .map(|(index, log_seq)| LogChunk {
                    log_seq: *log_seq,
                    stream: OutputStream::Stdout,
                    seq: index as u64,
                    data: format!("c{index}").into_bytes(),
                })
                .collect(),
            visible: Mutex::new(visible),
            page_chunks,
            longest_wait_ms: Mutex::new(0),
            marks_exit: false,
            polls: Mutex::new(0),
            failing_reads: Mutex::new(0),
        }
    }

    pub fn marking_exit(mut self) -> Self {
        self.marks_exit = true;
        self
    }

    pub fn failing(self, reads: u32) -> Self {
        Self {
            failing_reads: Mutex::new(reads),
            ..self
        }
    }

    pub fn share(self) -> Arc<Self> {
        Arc::new(self)
    }

    pub fn longest_wait_ms(&self) -> u64 {
        self.longest_wait_ms.lock().map(|value| *value).unwrap_or(0)
    }

    pub fn polls(&self) -> u32 {
        self.polls.lock().map(|value| *value).unwrap_or(0)
    }

    fn read_logs(&self, request: &ReadLogsRequest) -> Result<Vec<u8>, BusError> {
        {
            let mut failing = lock(&self.failing_reads)?;
            if *failing > 0 {
                *failing -= 1;
                return Err(BusError::Custom("bus down".into()));
            }
        }
        let mut visible = lock(&self.visible)?;
        {
            let mut longest = lock(&self.longest_wait_ms)?;
            *longest = (*longest).max(request.wait_ms.unwrap_or(0));
        }
        // Like the broker: the marker is read before the chunks.
        let exited = self.marks_exit && *visible == self.chunks.len();
        let mut page: Vec<LogChunk> = self.chunks[..*visible]
            .iter()
            .filter(|chunk| chunk.log_seq > request.after_seq)
            .cloned()
            .collect();
        let has_more = page.len() > self.page_chunks;
        page.truncate(self.page_chunks);
        if page.is_empty() && request.wait_ms.is_some() && *visible < self.chunks.len() {
            *visible += 1;
        }
        let next_seq = page.last().map_or(request.after_seq, |chunk| chunk.log_seq);
        serde_json::to_vec(&ReadLogsResponse::Logs {
            chunks: page,
            next_seq,
            has_more: LogsContinuation::from(has_more),
            truncated: OutputTruncation::Complete,
            exited: CommandExitReport::from(exited),
        })
        .map_err(custom)
    }

    fn poll_command(&self) -> Result<Vec<u8>, BusError> {
        *lock(&self.polls)? += 1;
        let exited = *lock(&self.visible)? == self.chunks.len();
        let response = if exited {
            PollCommandResponse::Completed(CommandOutput {
                exit_code: 0,
                stdout: vec![],
                stderr: vec![],
                timed_out: Default::default(),
                truncated: OutputTruncation::Complete,
            })
        } else {
            PollCommandResponse::Running
        };
        serde_json::to_vec(&response).map_err(custom)
    }
}

pub struct FakeLogBus(pub Arc<FakeLogBroker>);

#[async_trait]
impl Bus for FakeLogBus {
    async fn request(
        &self,
        subject: &str,
        payload: Bytes,
        _timeout: Duration,
        _headers: Option<async_nats::HeaderMap>,
    ) -> Result<Option<Bytes>, BusError> {
        let body = if subject.contains("read_logs") {
            let request: ReadLogsRequest = serde_json::from_slice(&payload).map_err(custom)?;
            self.0.read_logs(&request)?
        } else {
            self.0.poll_command()?
        };
        Ok(Some(Bytes::from(body)))
    }

    async fn subscribe(&self, _subject: &str) -> Result<BoxStream<'static, BusMessage>, BusError> {
        Ok(Box::pin(futures::stream::empty()))
    }
}

pub fn follow_client(broker: &Arc<FakeLogBroker>) -> SandboxClient {
    SandboxClient::from_bus(Box::new(FakeLogBus(broker.clone())))
}

pub fn execution() -> ExecutionId {
    ExecutionId::new("exec-1")
}

/// Every item of a follow from `after`, with errors rendered as text.
pub async fn collect_all(client: &SandboxClient, after: u64) -> Vec<Result<LogFollowEvent, String>> {
    let lease_id = LeaseId::new("lease-1");
    client
        .follow_logs(&lease_id, execution(), after)
        .map(|item: Result<LogFollowEvent, ClientError>| item.map_err(|error| error.to_string()))
        .collect()
        .await
}

/// The `log_seq` of every chunk of a follow from `after`, and the last non-chunk event.
pub async fn collect_follow(client: &SandboxClient, after: u64) -> Result<(Vec<u64>, Option<LogFollowEvent>), String> {
    let mut seen = vec![];
    let mut last = None;
    for item in collect_all(client, after).await {
        match item? {
            LogFollowEvent::Chunk(chunk) => seen.push(chunk.log_seq),
            other => last = Some(other),
        }
    }
    Ok((seen, last))
}
