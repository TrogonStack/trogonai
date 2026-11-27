//! Denials reported by the kernel, read back from the unified log.
//!
//! Every deny rule of a profile carries the launch's tag as its message, and
//! the kernel logs the message after the denial, so a `log stream` whose
//! predicate ends with the tag sees only this launch's denials. The stream
//! starts after the process may have, so the first few denials can be missed:
//! the monitor is a diagnostic, not part of the enforcement.

use std::net::{IpAddr, SocketAddr};
use std::process::Stdio;
use std::time::SystemTime;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc::UnboundedSender;
use trogon_sandbox_contract::LiteralPath;
use trogon_sandbox_contract::{EgressDecision, EgressViolation, EgressViolationReason, FsOperation, LeaseId};

use super::sbpl::LogTag;
use crate::fs_violation::{FsDenial, FsDenialSink};

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ViolationKind {
    FileRead { path: String },
    FileWrite { path: String },
    NetworkConnect { ip: Option<IpAddr>, port: u16 },
    Other { detail: String },
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SeatbeltViolation {
    pub process: String,
    pub pid: u32,
    pub operation: String,
    pub kind: ViolationKind,
}

impl SeatbeltViolation {
    /// The denial of a file operation in the shape the Linux backend reports
    /// its own, or `None` when the path is not one a lease can be told about.
    pub fn to_fs_denial(&self) -> Option<FsDenial> {
        let (path, operation) = match &self.kind {
            ViolationKind::FileRead { path } => (path, FsOperation::Read),
            ViolationKind::FileWrite { path } => (path, fs_write_operation(&self.operation)),
            _ => return None,
        };
        Some(FsDenial {
            path: LiteralPath::new(path).ok()?,
            operation,
        })
    }

    /// The denial of an outbound connection in the shape the egress proxy
    /// reports its own. The destination is a literal address: a sandbox that
    /// reached the kernel check had no proxy to name a host.
    pub fn to_egress_violation(&self, lease_id: LeaseId, at: SystemTime) -> Option<EgressViolation> {
        match &self.kind {
            ViolationKind::NetworkConnect { ip, port } => Some(EgressViolation {
                lease_id,
                host: None,
                port: *port,
                dest_ip: *ip,
                decision: EgressDecision::Deny,
                reason: EgressViolationReason::PolicyDenied,
                binding: None,
                request: None,
                at,
            }),
            _ => None,
        }
    }
}

/// One line of `log stream --style compact`, or `None` when it is not a
/// denial, is the tag line that follows one, or is known noise.
pub fn parse_line(line: &str) -> Option<SeatbeltViolation> {
    let report = line.split_once(SANDBOX_MARKER)?.1;
    if NOISE.iter().any(|noise| report.contains(noise)) {
        return None;
    }
    let (process_part, rest) = report.split_once(") deny(")?;
    let (process, pid) = process_part.rsplit_once('(')?;
    let pid: u32 = pid.parse().ok()?;
    let (_, rest) = rest.split_once(") ")?;
    let (operation, detail) = match rest.split_once(' ') {
        Some((operation, detail)) => (operation, detail.trim()),
        None => (rest.trim(), ""),
    };
    let kind = if operation.starts_with("file-read") || operation == "file-test-existence" {
        ViolationKind::FileRead {
            path: detail.to_string(),
        }
    } else if operation.starts_with("file-write") {
        ViolationKind::FileWrite {
            path: detail.to_string(),
        }
    } else if operation == "network-outbound" {
        match parse_endpoint(detail) {
            Some((ip, port)) => ViolationKind::NetworkConnect { ip, port },
            None => ViolationKind::Other {
                detail: detail.to_string(),
            },
        }
    } else {
        ViolationKind::Other {
            detail: detail.to_string(),
        }
    };
    Some(SeatbeltViolation {
        process: process.to_string(),
        pid,
        operation: operation.to_string(),
        kind,
    })
}

fn fs_write_operation(operation: &str) -> FsOperation {
    match operation {
        "file-write-create" => FsOperation::Create,
        "file-write-unlink" => FsOperation::Delete,
        "file-write-mode" | "file-write-owner" | "file-write-flags" | "file-write-times" | "file-write-xattr"
        | "file-write-acl" | "file-write-setugid" => FsOperation::SetMetadata,
        _ => FsOperation::Write,
    }
}

fn parse_endpoint(detail: &str) -> Option<(Option<IpAddr>, u16)> {
    let endpoint = detail.strip_prefix("remote:").unwrap_or(detail);
    if let Ok(addr) = endpoint.parse::<SocketAddr>() {
        return Some((Some(addr.ip()), addr.port()));
    }
    let (host, port) = endpoint.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let ip = host.trim_matches(['[', ']']).parse::<IpAddr>().ok();
    Some((ip, port))
}

fn predicate(tag: &LogTag) -> Option<String> {
    tag.as_str()
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
        .then(|| format!("eventMessage ENDSWITH \"{}\"", tag.as_str()))
}

pub struct ViolationMonitor {
    child: Child,
}

impl ViolationMonitor {
    /// Starts streaming this tag's denials into `sink`. Returns `None` when
    /// the tag cannot be put in a predicate safely or `log` cannot start.
    pub fn start(
        tag: &LogTag,
        violations: Option<UnboundedSender<SeatbeltViolation>>,
        fs_denials: Option<FsDenialSink>,
    ) -> Option<Self> {
        let predicate = predicate(tag)?;
        let mut child = Command::new(LOG_BINARY)
            .args(["stream", "--predicate", &predicate, "--style", "compact"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .ok()?;
        let stdout = child.stdout.take()?;
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Some(violation) = parse_line(&line) else {
                    continue;
                };
                let denial_gone = match (&fs_denials, violation.to_fs_denial()) {
                    (Some(sink), Some(denial)) => sink.send(denial).is_err(),
                    _ => false,
                };
                let violation_gone = violations.as_ref().is_some_and(|sink| sink.send(violation).is_err());
                let nothing_listens = (violations.is_none() || violation_gone) && (fs_denials.is_none() || denial_gone);
                if nothing_listens {
                    break;
                }
            }
        });
        Some(Self { child })
    }

    pub async fn stop(mut self) {
        let _ = self.child.kill().await;
    }
}

#[cfg(test)]
mod tests;
use super::constants::{LOG_BINARY, NOISE, SANDBOX_MARKER};
