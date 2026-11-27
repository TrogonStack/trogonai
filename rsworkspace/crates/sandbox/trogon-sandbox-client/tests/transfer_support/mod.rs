//! A broker whose sandbox is a host directory, for the file and directory transfer tests.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use futures::stream::BoxStream;
use tempfile::TempDir;
use trogon_sandbox_client::{Bus, BusError, BusMessage, SandboxClient};
use trogon_sandbox_contract::{Base64Data, FileOp, FileOpRequest, FileOpResponse, FileReadEnd, LeaseId};
use trogon_sandbox_file_ops::FsRoot;

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Rewrites a reply on its way back to the client, and may act on the request first.
pub type Tamper = Box<dyn Fn(&FileOp, FileOpResponse) -> FileOpResponse + Send + Sync>;

struct DirBus {
    root: FsRoot,
    ops: Arc<Mutex<Vec<FileOp>>>,
    tamper: Option<Tamper>,
}

fn custom(error: impl std::error::Error + Send + Sync + 'static) -> BusError {
    BusError::Custom(Box::new(error))
}

#[async_trait]
impl Bus for DirBus {
    async fn request(
        &self,
        _subject: &str,
        payload: Bytes,
        _timeout: Duration,
        _headers: Option<async_nats::HeaderMap>,
    ) -> Result<Option<Bytes>, BusError> {
        let request: FileOpRequest = serde_json::from_slice(&payload).map_err(custom)?;
        self.ops
            .lock()
            .map_err(|_| BusError::Custom("ops log poisoned".into()))?
            .push(request.op.clone());
        let mut reply = self.root.execute(&request.op);
        if let Some(tamper) = &self.tamper {
            reply = tamper(&request.op, reply);
        }
        Ok(Some(Bytes::from(serde_json::to_vec(&reply).map_err(custom)?)))
    }

    async fn subscribe(&self, _subject: &str) -> Result<BoxStream<'static, BusMessage>, BusError> {
        Ok(Box::pin(futures::stream::empty()))
    }
}

pub struct Harness {
    pub sandbox: TempDir,
    pub local: TempDir,
    pub client: SandboxClient,
    pub ops: Arc<Mutex<Vec<FileOp>>>,
    pub lease: LeaseId,
}

impl Harness {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::build(None)
    }

    pub fn tampering(
        tamper: impl Fn(&FileOp, FileOpResponse) -> FileOpResponse + Send + Sync + 'static,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::build(Some(Box::new(tamper)))
    }

    fn build(tamper: Option<Tamper>) -> Result<Self, Box<dyn std::error::Error>> {
        let sandbox = TempDir::new()?;
        let ops = Arc::new(Mutex::new(Vec::new()));
        let bus = DirBus {
            root: FsRoot::new(sandbox.path(), None)?,
            ops: ops.clone(),
            tamper,
        };
        Ok(Self {
            sandbox,
            local: TempDir::new()?,
            client: SandboxClient::from_bus(Box::new(bus)),
            ops,
            lease: LeaseId::new("lease-1"),
        })
    }

    /// `(offset, truncates)` of every write the client sent, in order.
    pub fn writes(&self) -> Vec<(u64, bool)> {
        let ops = self.ops.lock().map(|ops| ops.clone()).unwrap_or_default();
        ops.iter()
            .filter_map(|op| match op {
                FileOp::Write { offset, truncate, .. } => Some((*offset, truncate.truncates())),
                _ => None,
            })
            .collect()
    }

    pub fn clear_ops(&self) {
        if let Ok(mut ops) = self.ops.lock() {
            ops.clear();
        }
    }
}

/// Deterministic bytes that are not periodic at the chunk size.
pub fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 31 + i / 7) % 251) as u8).collect()
}

/// Applies `change` to the bytes, total size, and end marker of a read reply. Other replies pass through.
pub fn with_content(
    reply: FileOpResponse,
    change: impl FnOnce(&mut Vec<u8>, &mut u64, &mut FileReadEnd),
) -> FileOpResponse {
    let FileOpResponse::Content {
        data,
        offset,
        mut total_size,
        mut eof,
        truncated,
    } = reply
    else {
        return reply;
    };
    let mut raw = STANDARD.decode(data.as_str()).unwrap_or_default();
    change(&mut raw, &mut total_size, &mut eof);
    match Base64Data::new(STANDARD.encode(&raw)) {
        Ok(data) => FileOpResponse::Content {
            data,
            offset,
            total_size,
            eof,
            truncated,
        },
        Err(_) => FileOpResponse::Renamed,
    }
}

/// Flips one byte of every read, to prove the integrity check notices.
pub fn corrupt_reads(_op: &FileOp, reply: FileOpResponse) -> FileOpResponse {
    with_content(reply, |raw, _, _| {
        if let Some(first) = raw.first_mut() {
            *first ^= 0xff;
        }
    })
}
