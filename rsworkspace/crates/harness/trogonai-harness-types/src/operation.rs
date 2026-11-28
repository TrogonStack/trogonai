//! The identity a subagent operation runs under.
//!
//! The value lives here, below the crate that resolves replays, because the
//! shape is the wire format a session file is written in and a session file is
//! read by more than one caller.

use std::fmt;

use crate::hash::ContentHash;

/// Who asked for an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperationSource {
    /// The model requested it through a tool call.
    Model,
    /// A person requested it.
    Human,
}

impl OperationSource {
    /// The one-letter tag the wire format spells this as.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Model => "m",
            Self::Human => "h",
        }
    }

    /// Reads a tag back, refusing anything the format does not define.
    #[must_use]
    pub fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "m" => Some(Self::Model),
            "h" => Some(Self::Human),
            _ => None,
        }
    }
}

/// What issued an operation identity.
///
/// The distinction is what an identity is worth. Only the manager counts
/// epochs across a whole session, so only a manager-issued identity can be
/// compared against another record and be expected to mean the same thing; a
/// process-local one is an older spelling that a reader may still meet in a
/// session file and must not treat as proof of anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperationAuthority {
    /// Issued by whichever process happened to be running.
    ProcessLocal,
    /// Issued by the subagent manager.
    Manager,
}

/// The marker a manager-issued identity carries in place of nothing.
const MANAGER_MARKER: &str = "2";

/// The prefix every operation identity starts with.
const PREFIX: &str = "trogonaiop";

/// How many characters a SHA-256 digest is written as.
const DIGEST_CHARS: usize = 64;

/// An operation identity, bound to the invocation that raised it.
///
/// Canonical by construction: the only ways to get one are to issue it or to
/// parse a string that is already the format, so a value that exists is a value
/// whose parts can be read without checking them again.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BoundOperationId {
    source: OperationSource,
    authority: OperationAuthority,
    epoch: u64,
    digest: String,
}

impl BoundOperationId {
    /// Issues an identity for an invocation, on the manager's authority.
    #[must_use]
    pub fn issued(invocation_id: &str, source: OperationSource, epoch: u64) -> Self {
        Self {
            source,
            authority: OperationAuthority::Manager,
            epoch,
            digest: invocation_digest(invocation_id),
        }
    }

    /// Reads an identity back, refusing anything that is not the format.
    ///
    /// The refusals are what stop a record from claiming more than it says: a
    /// digest that is not exactly sixty-four lowercase hex characters is not a
    /// digest anything here produced, and an epoch written with a leading zero
    /// is a second spelling of a number that already has one, which would let
    /// two records name the same operation without comparing equal.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let mut parts = value.split(':');
        if parts.next()? != PREFIX {
            return None;
        }
        let second = parts.next()?;
        let manager_issued = second == MANAGER_MARKER;
        let source_tag = if manager_issued { parts.next()? } else { second };
        let epoch_raw = parts.next()?;
        let digest = parts.next()?;
        if parts.next().is_some() || digest.len() != DIGEST_CHARS {
            return None;
        }
        if epoch_raw.is_empty() || (epoch_raw.len() > 1 && epoch_raw.starts_with('0')) {
            return None;
        }
        if !digest.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')) {
            return None;
        }
        Some(Self {
            source: OperationSource::from_tag(source_tag)?,
            authority: if manager_issued {
                OperationAuthority::Manager
            } else {
                OperationAuthority::ProcessLocal
            },
            epoch: epoch_raw.parse().ok()?,
            digest: digest.to_owned(),
        })
    }

    /// Who asked for the operation.
    #[must_use]
    pub const fn source(&self) -> OperationSource {
        self.source
    }

    /// What issued the identity.
    #[must_use]
    pub const fn authority(&self) -> OperationAuthority {
        self.authority
    }

    /// Which issuance this was.
    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The digest of the invocation identifier this was bound to.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Whether this identity was issued for that invocation.
    ///
    /// A process-local identity never matches. Its epoch was counted by a
    /// process that is gone, so agreeing about the digest would say only that
    /// the same identifier was seen twice, which is not what a caller asking
    /// this question wants to know.
    ///
    /// Every caller has the identity in hand and can read the source off it, so the comparison
    /// belongs to whoever cares rather than to this.
    #[must_use]
    pub fn matches_invocation(&self, invocation_id: &str) -> bool {
        self.authority == OperationAuthority::Manager && self.digest == invocation_digest(invocation_id)
    }
}

impl fmt::Display for BoundOperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{PREFIX}:")?;
        if self.authority == OperationAuthority::Manager {
            write!(formatter, "{MANAGER_MARKER}:")?;
        }
        write!(formatter, "{}:{}:{}", self.source.tag(), self.epoch, self.digest)
    }
}

/// The digest an invocation identifier is recorded as.
fn invocation_digest(invocation_id: &str) -> String {
    ContentHash::of(invocation_id.as_bytes()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issued() -> BoundOperationId {
        BoundOperationId::issued("q-call", OperationSource::Model, 7)
    }

    #[test]
    fn an_issued_identity_reads_back_as_what_was_issued() {
        let rendered = issued().to_string();
        let parsed = BoundOperationId::parse(&rendered).expect("an identity trogonai just wrote");

        assert_eq!(parsed, issued());
        assert_eq!(parsed.source(), OperationSource::Model);
        assert_eq!(parsed.authority(), OperationAuthority::Manager);
        assert_eq!(parsed.epoch(), 7);
        assert!(rendered.starts_with("trogonaiop:2:m:7:"));
    }

    #[test]
    fn the_identifier_itself_is_not_in_the_identity() {
        let rendered = issued().to_string();

        assert!(!rendered.contains("q-call"));
        assert_eq!(issued().digest().len(), DIGEST_CHARS);
    }

    #[test]
    fn an_identity_is_bound_to_the_one_invocation_that_raised_it() {
        assert!(issued().matches_invocation("q-call"));
        assert!(!issued().matches_invocation("z-call"));
    }

    #[test]
    fn an_older_identity_without_the_manager_marker_still_reads() {
        let digest = invocation_digest("q-call");
        let parsed = BoundOperationId::parse(&format!("trogonaiop:h:3:{digest}"))
            .expect("the spelling a session file may already hold");

        assert_eq!(parsed.authority(), OperationAuthority::ProcessLocal);
        assert_eq!(parsed.source(), OperationSource::Human);
        assert_eq!(parsed.to_string(), format!("trogonaiop:h:3:{digest}"));
    }

    #[test]
    fn nobody_but_the_manager_can_prove_an_invocation_ran() {
        let digest = invocation_digest("q-call");
        let parsed = BoundOperationId::parse(&format!("trogonaiop:m:3:{digest}")).expect("a process-local id");

        assert!(!parsed.matches_invocation("q-call"));
    }

    #[test]
    fn what_is_not_the_format_is_refused() {
        let digest = invocation_digest("q-call");
        for refused in [
            String::new(),
            "trogonaiop".to_owned(),
            format!("op:2:m:1:{digest}"),
            format!("trogonaiop:2:x:1:{digest}"),
            format!("trogonaiop:2:m::{digest}"),
            format!("trogonaiop:2:m:01:{digest}"),
            format!("trogonaiop:2:m:1:{}", &digest[..63]),
            format!("trogonaiop:2:m:1:{}", digest.to_uppercase()),
            format!("trogonaiop:2:m:1:{digest}:extra"),
            format!("trogonaiop:2:m:99999999999999999999:{digest}"),
        ] {
            assert!(
                BoundOperationId::parse(&refused).is_none(),
                "{refused} is not an operation identity"
            );
        }
    }

    #[test]
    fn a_zero_epoch_parses_and_is_left_for_the_caller_to_judge() {
        let digest = invocation_digest("q-call");
        let parsed = BoundOperationId::parse(&format!("trogonaiop:2:m:0:{digest}"))
            .expect("zero is a number the format can write");

        assert_eq!(parsed.epoch(), 0);
    }
}
