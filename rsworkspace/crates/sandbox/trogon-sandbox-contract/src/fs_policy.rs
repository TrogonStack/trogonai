use serde::{Deserialize, Serialize};

use crate::fs_policy_wire::FsPolicyWire;
use crate::{FsPathEntry, FsPolicyError, FsPolicyList};

/// What a sandbox may read and write, as lists of guest paths.
///
/// Reads are open unless `deny_read` covers a path, and `allow_read_within_deny` entries can re-open a denied path.
/// A `credential_deny_read` entry is a read deny that no `allow_read_within_deny` entry can re-open, the files
/// that credential bindings keep from the sandbox. Only an `own_allow_within_deny` entry, a file the sandbox
/// runtime itself needs the command to read, can. Writes are closed unless `allow_write` covers a path, and
/// `deny_write` wins over it. Reading and writing are independent.
///
/// Each list holds at most [`FsPolicy::MAX_ENTRIES_PER_LIST`] entries. A deny glob ending in a separator can match
/// nothing and would fail open, so `deny_read`, `credential_deny_read`, and `deny_write` refuse it; the same glob in
/// an allow list only fails closed and is accepted. Construction and deserialization both enforce these rules, and
/// unknown fields are refused. The wire omits an empty list.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "FsPolicyWire", into = "FsPolicyWire")]
pub struct FsPolicy {
    deny_read: Vec<FsPathEntry>,
    credential_deny_read: Vec<FsPathEntry>,
    allow_read_within_deny: Vec<FsPathEntry>,
    own_allow_within_deny: Vec<FsPathEntry>,
    allow_write: Vec<FsPathEntry>,
    deny_write: Vec<FsPathEntry>,
}

impl FsPolicy {
    /// Most entries one list may hold.
    pub const MAX_ENTRIES_PER_LIST: usize = 1024;

    pub fn new(
        deny_read: Vec<FsPathEntry>,
        credential_deny_read: Vec<FsPathEntry>,
        allow_read_within_deny: Vec<FsPathEntry>,
        own_allow_within_deny: Vec<FsPathEntry>,
        allow_write: Vec<FsPathEntry>,
        deny_write: Vec<FsPathEntry>,
    ) -> Result<Self, FsPolicyError> {
        let lists = [
            (FsPolicyList::DenyRead, &deny_read),
            (FsPolicyList::CredentialDenyRead, &credential_deny_read),
            (FsPolicyList::AllowReadWithinDeny, &allow_read_within_deny),
            (FsPolicyList::OwnAllowWithinDeny, &own_allow_within_deny),
            (FsPolicyList::AllowWrite, &allow_write),
            (FsPolicyList::DenyWrite, &deny_write),
        ];
        for (list, entries) in lists {
            if entries.len() > Self::MAX_ENTRIES_PER_LIST {
                return Err(FsPolicyError::TooManyEntries {
                    list,
                    max: Self::MAX_ENTRIES_PER_LIST,
                    got: entries.len(),
                });
            }
        }
        for (list, entries) in [
            (FsPolicyList::DenyRead, &deny_read),
            (FsPolicyList::CredentialDenyRead, &credential_deny_read),
            (FsPolicyList::DenyWrite, &deny_write),
        ] {
            for entry in entries {
                if let FsPathEntry::Glob { pattern } = entry
                    && pattern.ends_with_separator()
                {
                    return Err(FsPolicyError::InertDenyGlob {
                        list,
                        pattern: pattern.as_str().to_string(),
                        suggestion: pattern.as_str().trim_end_matches('/').to_string(),
                    });
                }
            }
        }
        Ok(Self {
            deny_read,
            credential_deny_read,
            allow_read_within_deny,
            own_allow_within_deny,
            allow_write,
            deny_write,
        })
    }

    pub fn deny_read(&self) -> &[FsPathEntry] {
        &self.deny_read
    }

    pub fn credential_deny_read(&self) -> &[FsPathEntry] {
        &self.credential_deny_read
    }

    pub fn allow_read_within_deny(&self) -> &[FsPathEntry] {
        &self.allow_read_within_deny
    }

    pub fn own_allow_within_deny(&self) -> &[FsPathEntry] {
        &self.own_allow_within_deny
    }

    pub fn allow_write(&self) -> &[FsPathEntry] {
        &self.allow_write
    }

    pub fn deny_write(&self) -> &[FsPathEntry] {
        &self.deny_write
    }

    /// Reports whether every list is empty, so the wire can leave the policy out.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

impl TryFrom<FsPolicyWire> for FsPolicy {
    type Error = FsPolicyError;

    fn try_from(wire: FsPolicyWire) -> Result<Self, Self::Error> {
        Self::new(
            wire.deny_read,
            wire.credential_deny_read,
            wire.allow_read_within_deny,
            wire.own_allow_within_deny,
            wire.allow_write,
            wire.deny_write,
        )
    }
}

impl From<FsPolicy> for FsPolicyWire {
    fn from(policy: FsPolicy) -> Self {
        Self {
            deny_read: policy.deny_read,
            credential_deny_read: policy.credential_deny_read,
            allow_read_within_deny: policy.allow_read_within_deny,
            own_allow_within_deny: policy.own_allow_within_deny,
            allow_write: policy.allow_write,
            deny_write: policy.deny_write,
        }
    }
}
