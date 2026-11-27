//! Helpers for the file verb tests.

#![allow(dead_code)]

use serde::Serialize;
use trogon_sandbox_client::SandboxClient;
use trogon_sandbox_contract::{FileOp, FileOpRequest, FileOpResponse};

use crate::common::{MockBus, Reply, SentLog};

/// A client whose bus answers each request with the next of `replies`, plus the log of requests.
pub fn client_scripted(replies: &[FileOpResponse]) -> Result<(SandboxClient, SentLog), serde_json::Error> {
    let script = replies.iter().map(Reply::json).collect::<Result<Vec<_>, _>>()?;
    let bus = MockBus::scripted(script);
    let log = bus.log();
    Ok((bus.into_client(), log))
}

/// A client whose bus answers every request with `reply`.
pub fn client_for(reply: &FileOpResponse) -> Result<(SandboxClient, SentLog), serde_json::Error> {
    client_scripted(std::slice::from_ref(reply))
}

/// The file operation carried by the only request in `log`.
pub fn sent_op(log: &SentLog) -> Result<FileOp, Box<dyn std::error::Error>> {
    let sent = log.only()?;
    let request: FileOpRequest = serde_json::from_slice(&sent.payload)?;
    Ok(request.op)
}

pub fn json_of(value: &impl Serialize) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::to_value(value)
}
