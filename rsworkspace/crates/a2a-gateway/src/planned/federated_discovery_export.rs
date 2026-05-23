//! §8 roadmap — Operator-signed cross-Account **`{prefix}.discover.>`** publication contract.
//!
//! **Related:** [`docs/A2A_FEDERATED_DISCOVERY_SKETCH.md`](../../../../docs/A2A_FEDERATED_DISCOVERY_SKETCH.md) ·
//! [`A2A_PENDING_DECISION.md`](../../../../A2A_PENDING_DECISION.md) §8 · [`A2A_PLAN.md`](../../../../A2A_PLAN.md) ·
//! [`A2A_TODO.md`](../../../../A2A_TODO.md)

/// Opaque wire envelope for an operator-signed NATS Account discover export.
///
/// JSON-shaped field layout without a `serde` dependency in `a2a-gateway`; parsing stays at a future
/// boundary crate. Contract: [`A2A_FEDERATED_DISCOVERY_SKETCH.md`](../../../../docs/A2A_FEDERATED_DISCOVERY_SKETCH.md)
/// §「Operator-signed Account export contract」.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryExportEnvelope {
    /// Base64-encoded signature over `payload_json`.
    pub signature_b64: String,
    /// Publisher Account public key id that signed the export (NATS JWT account nkey).
    pub issuer_account_pubkey_id: String,
    /// Opaque JSON payload describing export subject `{prefix}.discover.>` and import allowlist.
    pub payload_json: String,
}

impl DiscoveryExportEnvelope {
    pub fn new(
        signature_b64: impl Into<String>,
        issuer_account_pubkey_id: impl Into<String>,
        payload_json: impl Into<String>,
    ) -> Self {
        Self {
            signature_b64: signature_b64.into(),
            issuer_account_pubkey_id: issuer_account_pubkey_id.into(),
            payload_json: payload_json.into(),
        }
    }
}

/// Stub seam for async verification of [`DiscoveryExportEnvelope`] against operator trust roots.
///
/// Future impls validate `signature_b64` over `payload_json` and export invariants from
/// [`A2A_FEDERATED_DISCOVERY_SKETCH.md`](../../../../docs/A2A_FEDERATED_DISCOVERY_SKETCH.md).
pub trait DiscoverExportVerifierFuture {
    type Error;
    type VerifyFuture: std::future::Future<Output = Result<(), Self::Error>> + Send;

    fn verify(&self, envelope: &DiscoveryExportEnvelope) -> Self::VerifyFuture;
}
