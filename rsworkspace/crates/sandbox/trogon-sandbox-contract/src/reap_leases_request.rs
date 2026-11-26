use serde::{Deserialize, Serialize};

use crate::reap_leases_request_wire::ReapLeasesRequestWire;
use crate::{ReapLeasesRequestError, ReapMode, SandboxTags, TenantId};

/// Client-facing request to release every lease of `tenant` that matches all the given filters.
///
/// At least one of `tags`, `older_than_seconds`, and `idle_for_seconds` must be set, so an empty request cannot release
/// a whole tenant by accident. Construction and deserialization both enforce it. In `DryRun` mode nothing is released
/// and the response only reports what would be. A repeat is safe: a lease that is already gone no longer matches.
/// `dry_run` is a [`ReapMode`], a JSON boolean, and reads as `Release` when omitted. `tags` and the two thresholds are
/// omitted when empty or absent.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ReapLeasesRequestWire", into = "ReapLeasesRequestWire")]
pub struct ReapLeasesRequest {
    tenant: TenantId,
    tags: SandboxTags,
    older_than_seconds: Option<u32>,
    idle_for_seconds: Option<u32>,
    dry_run: ReapMode,
}

impl ReapLeasesRequest {
    pub fn new(
        tenant: TenantId,
        tags: SandboxTags,
        older_than_seconds: Option<u32>,
        idle_for_seconds: Option<u32>,
        dry_run: ReapMode,
    ) -> Result<Self, ReapLeasesRequestError> {
        if tags.is_empty() && older_than_seconds.is_none() && idle_for_seconds.is_none() {
            return Err(ReapLeasesRequestError::NoFilter);
        }
        Ok(Self {
            tenant,
            tags,
            older_than_seconds,
            idle_for_seconds,
            dry_run,
        })
    }

    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    /// Matches leases whose tags contain every entry here.
    pub fn tags(&self) -> &SandboxTags {
        &self.tags
    }

    /// Matches leases created at least this many seconds ago.
    pub fn older_than_seconds(&self) -> Option<u32> {
        self.older_than_seconds
    }

    /// Matches leases with no activity for at least this many seconds, counting from creation when none was seen.
    pub fn idle_for_seconds(&self) -> Option<u32> {
        self.idle_for_seconds
    }

    pub fn dry_run(&self) -> ReapMode {
        self.dry_run
    }
}

impl TryFrom<ReapLeasesRequestWire> for ReapLeasesRequest {
    type Error = ReapLeasesRequestError;

    fn try_from(wire: ReapLeasesRequestWire) -> Result<Self, Self::Error> {
        Self::new(
            wire.tenant,
            wire.tags,
            wire.older_than_seconds,
            wire.idle_for_seconds,
            wire.dry_run,
        )
    }
}

impl From<ReapLeasesRequest> for ReapLeasesRequestWire {
    fn from(request: ReapLeasesRequest) -> Self {
        Self {
            tenant: request.tenant,
            tags: request.tags,
            older_than_seconds: request.older_than_seconds,
            idle_for_seconds: request.idle_for_seconds,
            dry_run: request.dry_run,
        }
    }
}
