//! Refusals (ADR#0085 Decision 5): a status the agent can act on plus a
//! machine-readable code in `x-trogon-egress-fault`.
//!
//! The body names the code and nothing else, so a refusal never leaks the
//! contents of a policy or of a request.

use std::fmt;

use crate::constants::{FAULT_HEADER, RETRY_AFTER_SECONDS};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fault {
    /// No permit of the calling agent names the destination.
    NoPermit,
    /// The destination resolves only to addresses the policy did not intend.
    AddressForbidden,
    /// The TLS ClientHello does not name the CONNECT host. Reported after the
    /// tunnel is open, so it reaches the audit trail but never a status line.
    SniMismatch,
    /// The policy snapshot cannot decide right now.
    PolicyUnavailable,
    /// The permitted destination could not be reached.
    UpstreamUnreachable,
    /// Only CONNECT is served.
    MethodNotAllowed,
    /// The request is not a well-formed CONNECT.
    BadRequest,
}

impl Fault {
    pub fn code(self) -> &'static str {
        match self {
            Self::NoPermit => "NO_PERMIT",
            Self::AddressForbidden => "ADDRESS_FORBIDDEN",
            Self::SniMismatch => "SNI_MISMATCH",
            Self::PolicyUnavailable => "POLICY_UNAVAILABLE",
            Self::UpstreamUnreachable => "UPSTREAM_UNREACHABLE",
            Self::MethodNotAllowed => "METHOD_NOT_ALLOWED",
            Self::BadRequest => "BAD_REQUEST",
        }
    }

    pub fn status(self) -> Status {
        match self {
            Self::NoPermit | Self::AddressForbidden | Self::SniMismatch => Status::Forbidden,
            Self::PolicyUnavailable => Status::ServiceUnavailable,
            Self::UpstreamUnreachable => Status::BadGateway,
            Self::MethodNotAllowed => Status::MethodNotAllowed,
            Self::BadRequest => Status::BadRequest,
        }
    }

    /// The complete HTTP/1.1 response for this refusal, ready to write.
    pub fn response(self) -> Vec<u8> {
        let status = self.status();
        let code = self.code();
        let body = format!("{code}\n");
        let mut head = format!(
            "HTTP/1.1 {} {}\r\n{FAULT_HEADER}: {code}\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n",
            status.code(),
            status.reason(),
            body.len(),
        );
        if status == Status::ServiceUnavailable {
            head.push_str(&format!("retry-after: {RETRY_AFTER_SECONDS}\r\n"));
        }
        if status == Status::MethodNotAllowed {
            head.push_str("allow: CONNECT\r\n");
        }
        head.push_str("\r\n");
        head.push_str(&body);
        head.into_bytes()
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    BadRequest,
    Forbidden,
    MethodNotAllowed,
    BadGateway,
    ServiceUnavailable,
}

impl Status {
    pub fn code(self) -> u16 {
        match self {
            Self::BadRequest => 400,
            Self::Forbidden => 403,
            Self::MethodNotAllowed => 405,
            Self::BadGateway => 502,
            Self::ServiceUnavailable => 503,
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::BadRequest => "Bad Request",
            Self::Forbidden => "Forbidden",
            Self::MethodNotAllowed => "Method Not Allowed",
            Self::BadGateway => "Bad Gateway",
            Self::ServiceUnavailable => "Service Unavailable",
        }
    }
}

#[cfg(test)]
mod tests;
