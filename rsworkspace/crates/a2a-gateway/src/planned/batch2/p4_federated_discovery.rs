//! Phase 4 — Federated discovery — operator-signed **`a2a.discover.>`** export; SpiceDB import gating.
//!
//! Cross-Account AgentCard discovery is **off by default**. Operators opt in by signing NATS Account
//! **exports** of `{prefix}.discover.>` on publisher Accounts and matching **imports** on consumer
//! Accounts. The gateway applies **SpiceDB catalog shaping** at the import boundary so callers only
//! see federated AgentCards they are authorized to view.
//!
//! **Related:**
//! - [A2A federated discovery sketch](../../../../docs/A2A_FEDERATED_DISCOVERY_SKETCH.md) — trust
//!   boundaries, operator export/import contract, KV vs imported discover, SpiceDB tuple sketch.
//! - [`super::super::federated_discovery_export`] — wire-shaped [`DiscoveryExportEnvelope`] and async
//!   verification seam for operator-signed export JWTs.
//! - [`super::p1_spicedb_gateway`] — [`BulkCatalogGate`] / [`BulkCheckPermission`] catalog shaping
//!   reused for federated agent ids at merge time.
//! - [`super::p0_agentcard_gateway_revalidation`] — [`AgentCardMaterializationPath::FederationImported`]
//!   JSON-Schema re-validation on bypass paths (no local KV mirror).
//! - [`../../../../A2A_TODO.md`](../../../../A2A_TODO.md) §Phase 4 — federation tracker.

use super::p1_spicedb_gateway::{BulkCheckOutcome, ConsistencyHint};
use super::super::federated_discovery_export::DiscoveryExportEnvelope;

/// Default NATS subject scope for operator-signed discover **service** exports (prefix `a2a`).
pub const DEFAULT_DISCOVER_EXPORT_SCOPE: &str = "a2a.discover.>";

/// Compile-time fingerprint of an operator-signed discover export (publisher Account nkey + scope).
///
/// Stable thumbprint for import-side policy: `"{pubkey_id}:{signed_scope}"` via [`Self::thumbprint`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DiscoverExportFingerprint {
    /// Publisher Account public key id (NATS JWT account nkey) that signed the export.
    pub pubkey_id: String,
    /// Operator-signed export subject scope (typically [`DEFAULT_DISCOVER_EXPORT_SCOPE`]).
    pub signed_scope: &'static str,
}

impl DiscoverExportFingerprint {
    pub fn new(pubkey_id: impl Into<String>, signed_scope: &'static str) -> Self {
        Self {
            pubkey_id: pubkey_id.into(),
            signed_scope,
        }
    }

    /// Stable import-boundary key for [`FederationImportGate::import_allowed`].
    pub fn thumbprint(&self) -> String {
        format!("{}:{}", self.pubkey_id, self.signed_scope)
    }
}

/// Import-boundary verdict before federated catalog merge or per-id discover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportVerdict {
    /// Proceed — federated discover replies may enter SpiceDB catalog shaping.
    Allow,
    /// Block entire federated catalog from this export (operator revoked import or policy deny).
    DenyCatalog,
}

impl ImportVerdict {
    pub fn is_allow(self) -> bool {
        matches!(self, Self::Allow)
    }
}

/// SpiceDB gating at the federation **import** boundary — evaluates configured export thumbprints.
///
/// Runs **before** gateway catalog merge ([`A2A_FEDERATED_DISCOVERY_SKETCH.md`](../../../../docs/A2A_FEDERATED_DISCOVERY_SKETCH.md)
/// §「SpiceDB at the federation boundary」). Denied imports never reach [`BulkCatalogGate`].
pub trait FederationImportGate: Send + Sync {
    fn import_allowed(&self, export_thumbprint: &str) -> ImportVerdict;
}

/// Operator-signed NATS Account export contract for cross-Account discover (compile-time reference).
///
/// Contract invariants from
/// [`A2A_FEDERATED_DISCOVERY_SKETCH.md`](../../../../docs/A2A_FEDERATED_DISCOVERY_SKETCH.md)
/// §「Operator-signed Account export contract」:
///
/// | Role | NATS JWT action | Subject |
/// |------|-----------------|---------|
/// | **Publisher** | Service **export** | `{prefix}.discover.>` |
/// | **Consumer** | Service **import** | `{prefix}.discover.>` from named publisher Account(s) |
///
/// Application code does **not** mint export JWTs — operators sign both sides via `nsc`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorDiscoverExportContract {
    /// Service export subject (default [`DEFAULT_DISCOVER_EXPORT_SCOPE`]).
    pub export_subject: &'static str,
    /// Explicit consumer Account public keys allowed to import (no wildcard in production).
    pub allowed_importer_pubkey_ids: Vec<String>,
}

impl OperatorDiscoverExportContract {
    pub fn a2a_default(allowed_importer_pubkey_ids: Vec<String>) -> Self {
        Self {
            export_subject: DEFAULT_DISCOVER_EXPORT_SCOPE,
            allowed_importer_pubkey_ids,
        }
    }

    /// Fingerprint for a publisher Account signing this contract.
    pub fn publisher_fingerprint(&self, publisher_pubkey_id: impl Into<String>) -> DiscoverExportFingerprint {
        DiscoverExportFingerprint::new(publisher_pubkey_id, self.export_subject)
    }
}

/// Wire envelope + fingerprint pair tracked on the consumer Account import boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederatedImportBinding {
    pub envelope: DiscoveryExportEnvelope,
    pub fingerprint: DiscoverExportFingerprint,
}

impl FederatedImportBinding {
    pub fn export_thumbprint(&self) -> String {
        self.fingerprint.thumbprint()
    }
}

/// Federated catalog merge stage — local KV ids plus allowed imported discovers after SpiceDB checks.
///
/// Flow from
/// [`A2A_FEDERATED_DISCOVERY_SKETCH.md`](../../../../docs/A2A_FEDERATED_DISCOVERY_SKETCH.md)
/// §「Client-visible catalog merge (gateway)」:
///
/// 1. Enumerate **local** agent ids.
/// 2. For each configured federation import passing [`FederationImportGate`], collect federated candidates.
/// 3. **`BulkCheckPermission`** — `user:{sub}` / `view` / `agent:{publisher_account}:{agent_id}`.
/// 4. Return shaped AgentCard list; omit or deny unauthorized federated entries.
pub trait FederatedCatalogMerge: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn merge_visible_agent_ids(
        &self,
        consistency: ConsistencyHint,
        local_agent_ids: &[String],
        federated_agent_ids: &[String],
    ) -> impl std::future::Future<Output = Result<Vec<BulkCheckOutcome>, Self::Error>> + Send;
}

/// Static import gate — allow-list of export thumbprints configured on the consumer Account.
#[derive(Debug, Default)]
pub struct AllowlistFederationImportGate {
    allowed_thumbprints: std::collections::HashSet<String>,
}

impl AllowlistFederationImportGate {
    pub fn from_bindings(bindings: &[FederatedImportBinding]) -> Self {
        Self {
            allowed_thumbprints: bindings
                .iter()
                .map(FederatedImportBinding::export_thumbprint)
                .collect(),
        }
    }

    pub fn allow_thumbprint(mut self, export_thumbprint: impl Into<String>) -> Self {
        self.allowed_thumbprints.insert(export_thumbprint.into());
        self
    }
}

impl FederationImportGate for AllowlistFederationImportGate {
    fn import_allowed(&self, export_thumbprint: &str) -> ImportVerdict {
        if self.allowed_thumbprints.contains(export_thumbprint) {
            ImportVerdict::Allow
        } else {
            ImportVerdict::DenyCatalog
        }
    }
}

/// No-op merge stub — returns empty outcomes until SpiceDB wiring lands.
#[derive(Debug, Default)]
pub struct NoopFederatedCatalogMerge;

impl FederatedCatalogMerge for NoopFederatedCatalogMerge {
    type Error = std::convert::Infallible;

    async fn merge_visible_agent_ids(
        &self,
        _consistency: ConsistencyHint,
        _local_agent_ids: &[String],
        _federated_agent_ids: &[String],
    ) -> Result<Vec<BulkCheckOutcome>, Self::Error> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_export_fingerprint_thumbprint_includes_scope() {
        let fp = DiscoverExportFingerprint::new("ABCD1234", DEFAULT_DISCOVER_EXPORT_SCOPE);
        assert_eq!(fp.thumbprint(), "ABCD1234:a2a.discover.>");
    }

    #[test]
    fn allowlist_gate_denies_unknown_export() {
        let gate = AllowlistFederationImportGate::default();
        assert_eq!(
            gate.import_allowed("UNKNOWN:a2a.discover.>"),
            ImportVerdict::DenyCatalog
        );
    }

    #[test]
    fn allowlist_gate_allows_configured_export() {
        let binding = FederatedImportBinding {
            envelope: DiscoveryExportEnvelope::new("sig", "PUBKEY", "{}"),
            fingerprint: DiscoverExportFingerprint::new("PUBKEY", DEFAULT_DISCOVER_EXPORT_SCOPE),
        };
        let gate = AllowlistFederationImportGate::from_bindings(&[binding]);
        assert_eq!(
            gate.import_allowed("PUBKEY:a2a.discover.>"),
            ImportVerdict::Allow
        );
    }

    #[test]
    fn operator_contract_default_scope_is_a2a_discover_wildcard() {
        let contract = OperatorDiscoverExportContract::a2a_default(vec!["CONSUMER".to_owned()]);
        assert_eq!(contract.export_subject, "a2a.discover.>");
        assert_eq!(
            contract.publisher_fingerprint("PUBLISHER").signed_scope,
            DEFAULT_DISCOVER_EXPORT_SCOPE
        );
    }
}
