use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::capture::Capture;
use super::constants::{MAX_CAPTURES, MAX_NAME_LEN};
use super::custody_error::SnapshotCustodyError;

/// Who owns the snapshots taken from one source sandbox, and whether the source may still take more.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CustodyRecord {
    version: u32,
    /// A digest of the Daytona API the source lives on.
    origin: String,
    pub(super) source: String,
    /// The organization the source belongs to, or `None` for a fence written before the source could be read.
    pub(super) organization: Option<String>,
    /// Set when the source is being cleaned up: no capture may start, and every unkept one is deleted.
    pub(super) closed: bool,
    pub(super) captures: BTreeMap<String, Capture>,
}

impl CustodyRecord {
    pub(super) fn new(origin: &str, source: &str, organization: Option<String>, closed: bool) -> Self {
        Self {
            version: 1,
            origin: origin.to_owned(),
            source: source.to_owned(),
            organization,
            closed,
            captures: BTreeMap::new(),
        }
    }

    pub(super) fn validate(&self, origin: &str, source: &str) -> Result<(), SnapshotCustodyError> {
        let organization_known = self.organization.as_deref().is_some_and(|id| !id.is_empty());
        let fence_only = self.closed && self.captures.is_empty();
        if self.version != 1
            || self.origin != origin
            || self.source != source
            || self.source.is_empty()
            || !(organization_known || fence_only)
            || self.captures.len() > MAX_CAPTURES
        {
            return Err(SnapshotCustodyError::InvalidIdentity);
        }
        for (name, capture) in &self.captures {
            validate_name(name)?;
            let bound = capture.id.as_deref().is_some_and(|id| !id.is_empty());
            let id_empty = capture.id.as_deref().is_some_and(str::is_empty);
            if id_empty || (capture.kept || capture.deleted) && !bound || capture.kept && capture.deleted {
                return Err(SnapshotCustodyError::InvalidState);
            }
        }
        Ok(())
    }
}

/// A snapshot name is a short path step.
pub(super) fn validate_name(name: &str) -> Result<(), SnapshotCustodyError> {
    let valid = !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && name != "."
        && name != "..";
    if valid {
        Ok(())
    } else {
        Err(SnapshotCustodyError::InvalidName)
    }
}
