//! The record the bridge writes for each request it handles (ADR#0082
//! Decision 6). The bridge presents the caller's credential on a connection of
//! its own, so every record names the caller, the bridge as the presenter, and
//! the `Trogon-Req-Id` the hops behind the bridge log the request under. That
//! is what lets a reader tell a bridged request from one a client sent on its
//! own connection.

use std::fmt;

use a2a_nats::ReqId;
use tracing::{info, warn};
use trogon_identity_types::SubjectPrincipal;

use crate::constants::BRIDGE_PRESENTER;
use crate::edge_auth::EdgeAuthMode;
use crate::edge_identity::CredentialSubject;
use crate::error::EdgeAuthError;

/// The caller a record names, by how far the bridge has verified it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordedCaller {
    /// The principal the auth callout minted for the request, which every
    /// subject the bridge publishes the request on carries.
    SubjectPrincipal(SubjectPrincipal),
    /// The caller whose proof of possession the edge verified, before anything
    /// was minted for the request.
    EdgeIdentity(CredentialSubject),
    /// Nobody the bridge has verified.
    Unverified,
}

impl RecordedCaller {
    /// The caller an edge failure still vouches for, which is only the one
    /// that signed a request whose bearer names somebody else.
    #[must_use]
    pub fn behind(error: &EdgeAuthError) -> Self {
        match error {
            EdgeAuthError::BearerMismatch { verified, .. } => Self::EdgeIdentity(verified.clone()),
            _ => Self::Unverified,
        }
    }

    /// Which kind of caller the record names, as its `caller_source`.
    #[must_use]
    pub fn source(&self) -> &'static str {
        match self {
            Self::SubjectPrincipal(_) => "subject_principal",
            Self::EdgeIdentity(_) => "edge_identity",
            Self::Unverified => "unverified",
        }
    }
}

impl fmt::Display for RecordedCaller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SubjectPrincipal(principal) => principal.fmt(f),
            Self::EdgeIdentity(subject) => subject.fmt(f),
            Self::Unverified => f.pad(self.source()),
        }
    }
}

/// What the bridge records about a request: who the caller is, that the
/// bridge presented the request, and the `Trogon-Req-Id` it travels under.
///
/// Every field but the edge mode is written as text, which a log line quotes
/// and escapes: the request id is the caller's own JSON-RPC id, a refusal
/// quotes the unverified bearer, and a stream payload is whatever an agent
/// published, so none of them may end the line it is logged on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgedRequestRecord {
    caller: RecordedCaller,
    req_id: ReqId,
}

impl BridgedRequestRecord {
    #[must_use]
    pub fn new(caller: RecordedCaller, req_id: ReqId) -> Self {
        Self { caller, req_id }
    }

    #[must_use]
    pub fn caller(&self) -> &RecordedCaller {
        &self.caller
    }

    /// The bridge, which holds the connection a request travels on whoever its
    /// caller is.
    #[must_use]
    pub fn presenter(&self) -> &'static str {
        BRIDGE_PRESENTER
    }

    #[must_use]
    pub fn req_id(&self) -> &ReqId {
        &self.req_id
    }

    pub(crate) fn shadowed_at_edge(&self, mode: EdgeAuthMode, error: &EdgeAuthError) {
        warn!(
            caller = self.caller.to_string().as_str(),
            caller_source = self.caller.source(),
            presenter = self.presenter(),
            req_id = self.req_id.as_str(),
            %mode,
            reason = error.detail().as_str(),
            "AAuth proof of possession failed and shadow mode admitted the request"
        );
    }

    pub(crate) fn refused_at_edge(&self, mode: EdgeAuthMode, error: &EdgeAuthError) {
        warn!(
            caller = self.caller.to_string().as_str(),
            caller_source = self.caller.source(),
            presenter = self.presenter(),
            req_id = self.req_id.as_str(),
            %mode,
            reason = error.detail().as_str(),
            "refused the request at the HTTP edge before minting for it"
        );
    }

    pub(crate) fn presented_to_gateway(&self) {
        info!(
            caller = self.caller.to_string().as_str(),
            caller_source = self.caller.source(),
            presenter = self.presenter(),
            req_id = self.req_id.as_str(),
            "presenting the request to the gateway on the caller's behalf"
        );
    }

    pub(crate) fn undecodable_stream_payload(&self, payload: &[u8]) {
        warn!(
            caller = self.caller.to_string().as_str(),
            caller_source = self.caller.source(),
            presenter = self.presenter(),
            req_id = self.req_id.as_str(),
            payload = &*String::from_utf8_lossy(payload),
            "stream payload is not a JSON-RPC envelope"
        );
    }
}

#[cfg(test)]
mod tests;
