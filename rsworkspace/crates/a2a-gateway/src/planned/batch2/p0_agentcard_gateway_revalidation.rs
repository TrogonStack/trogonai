//! Phase 0 — Gateway AgentCard JSON-Schema re-validation on read (non-KV paths).
//!
//! Tracked in [`../../../../A2A_TODO.md`](../../../../A2A_TODO.md) Phase 0 (catalog item:
//! gateway / edge re-validates AgentCard JSON-Schema on read once cards are materialized
//! **outside NATS KV**). See also [`../../../../docs/A2A_GATEWAY_ROADMAP.md`](../../../../docs/A2A_GATEWAY_ROADMAP.md)
//! (AgentCard validation at gateway) and [`../../../../docs/catalog-kv-watch.md`](../../../../docs/catalog-kv-watch.md)
//! (KV registrar ingress + [`KvCatalogStore`](a2a_nats::catalog::KvCatalogStore) get/put contract).
//!
//! ## Why re-validate at the gateway?
//!
//! The canonical AgentCard JSON Schema ships in **`a2a-pack`**
//! ([`agent_card_schema`](https://docs.rs/a2a-pack/latest/a2a_pack/agent_card_schema/index.html),
//! bundled `schemas/agent-card.min.json`, draft-07 aligned with **`a2a-types` 0.2**
//! `supportedInterfaces`). [`KvCatalogStore`](a2a_nats::catalog::KvCatalogStore) already applies
//! that schema on **KV get/put** (registrar write + defense-in-depth read). Bypass paths — edge CDN
//! caches, federation import replies, HTTPS bridge card materialization — can surface bytes that
//! never crossed the KV boundary; the gateway must re-run the same schema check before trusting
//! them for discover responses or downstream RPC routing.
//!
//! ## Read-path flow (non-KV materialization)
//!
//! ```text
//! 1. Caller requests an AgentCard (discover RPC, federation list, bridge well-known adapter, …).
//! 2. Gateway resolves how bytes will be materialized → [`AgentCardMaterializationPath`].
//! 3. KvBacked:
//!    - Read via [`KvCatalogStore::get_card`] (or equivalent discover service backed by KV).
//!    - Schema already enforced at the KV store boundary — skip gateway re-validation.
//! 4. HttpsBridgeCache | FederationImported:
//!    - Fetch raw JSON bytes from the bypass source (CDN object, cross-Account import reply, …).
//!    - Invoke [`AgentCardIngressValidator::validate_json`] (production: delegate to
//!      `a2a_pack::AgentCardJsonSchema::bundled()` / `validate_agent_card_value`).
//!    - On failure: reject / omit card (fail-closed); increment
//!      [`METRIC_AGENTCARD_REVALIDATION_FAILED_TOTAL`].
//!    - On success: deserialize to domain type in the caller layer (outside this seam) and return.
//! 5. Emit [`METRIC_AGENTCARD_REVALIDATION_TOTAL`] (attempted) and
//!    [`METRIC_AGENTCARD_REVALIDATION_SKIPPED_TOTAL`] when step 3 applies.
//! ```
//!
//! **Status:** compile-only seam — no wiring into [`crate::runtime`] yet. Production validator
//! implementation will live in a thin adapter over **`a2a-pack`** (not duplicated here).

use std::fmt;

/// OpenTelemetry-style counter: gateway AgentCard re-validation attempts (non-KV paths only).
pub const METRIC_AGENTCARD_REVALIDATION_TOTAL: &str = "a2a.gateway.agentcard.revalidation.total";

/// Counter: re-validation failures (schema reject) on bypass paths.
pub const METRIC_AGENTCARD_REVALIDATION_FAILED_TOTAL: &str =
    "a2a.gateway.agentcard.revalidation.failed_total";

/// Counter: reads that skipped gateway re-validation because KV already validated
/// ([`AgentCardMaterializationPath::KvBacked`]).
pub const METRIC_AGENTCARD_REVALIDATION_SKIPPED_TOTAL: &str =
    "a2a.gateway.agentcard.revalidation.skipped_total";

/// Label value for [`METRIC_AGENTCARD_REVALIDATION_TOTAL`] / failure counters (`path` attribute).
pub const METRIC_LABEL_PATH: &str = "path";

/// Stable label identifying a bypass / materialization path in metrics and logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BypassPath {
    /// Short static identifier exported to metrics (`path` label) and structured logs.
    pub label: &'static str,
}

/// Where gateway code materialized AgentCard JSON bytes before handing them to callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentCardMaterializationPath {
    /// JetStream KV / [`KvCatalogStore`](a2a_nats::catalog::KvCatalogStore) — schema enforced at store boundary.
    KvBacked,
    /// HTTPS↔NATS bridge sidecar cache or CDN edge object ([`super::super::bridge_https_sidecar`]).
    HttpsBridgeCache,
    /// Cross-Account federation import ([`super::p4_federated_discovery`]) — no local KV mirror.
    FederationImported,
}

impl AgentCardMaterializationPath {
    /// Metrics/log label for this materialization path.
    pub fn bypass_path(self) -> BypassPath {
        match self {
            Self::KvBacked => BypassPath {
                label: "kv_backed",
            },
            Self::HttpsBridgeCache => BypassPath {
                label: "https_bridge_cache",
            },
            Self::FederationImported => BypassPath {
                label: "federation_imported",
            },
        }
    }

    /// Whether the gateway must run JSON-Schema validation before trusting bytes.
    ///
    /// [`Self::KvBacked`] returns `false` because [`KvCatalogStore`] already validates on get/put
    /// using the bundled **`a2a-pack`** schema.
    pub fn requires_gateway_revalidation(self) -> bool {
        !matches!(self, Self::KvBacked)
    }
}

/// Validates raw AgentCard JSON bytes against the canonical **`a2a-pack`** JSON Schema.
///
/// Production implementations parse JSON internally (via `serde_json` inside **`a2a-pack`**) and
/// call `AgentCardJsonSchema::bundled().validate(...)`. This trait stays byte-oriented so gateway
/// ingress can validate cache/import payloads before deserializing to domain types.
pub trait AgentCardIngressValidator {
    /// Validation error surfaced to gateway discover/import handlers.
    type Error: fmt::Display + std::error::Error + Send + Sync + 'static;

    /// Run JSON-Schema validation on untrusted AgentCard bytes.
    fn validate_json(&self, bytes: &[u8]) -> Result<(), Self::Error>;
}

/// Gateway read-path policy: re-validate bypass materializations, trust KV reads.
#[derive(Debug, Clone, Copy)]
pub struct AgentCardGatewayRevalidation<V> {
    validator: V,
}

impl<V> AgentCardGatewayRevalidation<V> {
    pub fn new(validator: V) -> Self {
        Self { validator }
    }

    pub fn validator(&self) -> &V {
        &self.validator
    }
}

impl<V> AgentCardGatewayRevalidation<V>
where
    V: AgentCardIngressValidator,
{
    /// Apply Phase 0 re-validation policy for a materialized AgentCard payload.
    pub fn revalidate_on_read(
        &self,
        path: AgentCardMaterializationPath,
        bytes: &[u8],
    ) -> Result<RevalidationOutcome, V::Error> {
        if !path.requires_gateway_revalidation() {
            return Ok(RevalidationOutcome::SkippedKvBacked {
                path: path.bypass_path(),
            });
        }

        self.validator.validate_json(bytes)?;
        Ok(RevalidationOutcome::Validated {
            path: path.bypass_path(),
            byte_len: bytes.len(),
        })
    }
}

/// Result of gateway read-path schema policy (for metrics + audit hooks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevalidationOutcome {
    /// KV path — [`KvCatalogStore`] already validated; gateway skipped re-check.
    SkippedKvBacked { path: BypassPath },
    /// Bypass path — bytes passed **`a2a-pack`** JSON Schema at gateway boundary.
    Validated { path: BypassPath, byte_len: usize },
}

impl fmt::Display for RevalidationOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SkippedKvBacked { path } => {
                write!(f, "agent card revalidation skipped (path={})", path.label)
            }
            Self::Validated { path, byte_len } => write!(
                f,
                "agent card revalidated (path={}, bytes={byte_len})",
                path.label
            ),
        }
    }
}

/// Compile-only validation error until **`a2a-pack`** adapter is wired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedAgentCardValidateError {
    /// Validator not connected to production **`a2a-pack`** schema yet.
    NotImplemented,
    /// Bypass path received empty payload.
    EmptyPayload { path: BypassPath },
}

impl fmt::Display for PlannedAgentCardValidateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotImplemented => {
                write!(f, "AgentCard ingress validator not wired to a2a-pack schema yet")
            }
            Self::EmptyPayload { path } => {
                write!(f, "empty AgentCard payload on bypass path {}", path.label)
            }
        }
    }
}

impl std::error::Error for PlannedAgentCardValidateError {}

/// Delegates to [`a2a_pack::validate_agent_card_on_read`] for bypass materializations.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlannedPackAgentCardIngressValidator;

impl AgentCardIngressValidator for PlannedPackAgentCardIngressValidator {
    type Error = PlannedAgentCardValidateError;

    fn validate_json(&self, bytes: &[u8]) -> Result<(), Self::Error> {
        if bytes.is_empty() {
            return Err(PlannedAgentCardValidateError::EmptyPayload {
                path: BypassPath {
                    label: "unspecified",
                },
            });
        }

        let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| {
            PlannedAgentCardValidateError::NotImplemented
        })?;
        a2a_pack::validate_agent_card_on_read(&value, a2a_pack::AgentCardSource::GatewaySurface)
            .map_err(|_| PlannedAgentCardValidateError::NotImplemented)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kv_backed_skips_gateway_revalidation() {
        let policy = AgentCardGatewayRevalidation::new(PlannedPackAgentCardIngressValidator);
        let outcome = policy
            .revalidate_on_read(AgentCardMaterializationPath::KvBacked, br#"{"name":"x"}"#)
            .expect("kv path should not invoke validator");

        assert_eq!(
            outcome,
            RevalidationOutcome::SkippedKvBacked {
                path: BypassPath {
                    label: "kv_backed",
                },
            }
        );
    }

    #[test]
    fn bypass_path_invokes_validator() {
        let policy = AgentCardGatewayRevalidation::new(PlannedPackAgentCardIngressValidator);
        let valid = br#"{"name":"remote","supportedInterfaces":[{"url":"https://example.com/a2a","protocolBinding":"JSONRPC","protocolVersion":"0.2.0"}]}"#;
        policy
            .revalidate_on_read(AgentCardMaterializationPath::FederationImported, valid)
            .expect("valid federated card passes gateway revalidation");

        let err = policy
            .revalidate_on_read(
                AgentCardMaterializationPath::FederationImported,
                br#"{"name":""}"#,
            )
            .expect_err("invalid federated card fails gateway revalidation");

        assert!(matches!(err, PlannedAgentCardValidateError::NotImplemented));
    }

    #[test]
    fn bypass_path_rejects_empty_payload() {
        let policy = AgentCardGatewayRevalidation::new(PlannedPackAgentCardIngressValidator);
        let err = policy
            .revalidate_on_read(AgentCardMaterializationPath::HttpsBridgeCache, b"")
            .expect_err("empty bypass payload should fail");

        assert!(matches!(
            err,
            PlannedAgentCardValidateError::EmptyPayload { .. }
        ));
    }

    #[test]
    fn materialization_path_labels_are_stable() {
        assert_eq!(
            AgentCardMaterializationPath::HttpsBridgeCache
                .bypass_path()
                .label,
            "https_bridge_cache"
        );
        assert!(AgentCardMaterializationPath::FederationImported.requires_gateway_revalidation());
        assert!(!AgentCardMaterializationPath::KvBacked.requires_gateway_revalidation());
    }
}
