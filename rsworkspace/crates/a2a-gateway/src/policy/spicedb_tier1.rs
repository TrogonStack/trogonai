//! SpiceDB-backed Tier-1 authorization gate.
//!
//! Gated behind the `spicedb` feature so deployments without an
//! external authoriser don't pay the authzed/tonic compile cost.
//! This slice ships the typed gate trait + Noop impl, the typed
//! outcome enum, the audit-side owner-tuple emitter trait, and the
//! env-driven layer config that hands callers either a Noop gate
//! (`A2A_GATEWAY_TIER1_SPICEDB_ENABLED` unset) or a runtime error
//! when an operator tries to enable Tier-1 without providing the
//! credentials.
//!
//! The `LiveSpiceDbTier1Gate` that talks to authzed/SpiceDB over
//! gRPC ships in a follow-up alongside the `LiveBulkImportPermissionClient`
//! integration smoke harness; this slice keeps the gateway boot
//! path's policy stack on the Noop gate so the slice can land
//! without dragging the live client wiring into review.

use std::sync::Arc;

use a2a_nats::A2aMethod;
use a2a_nats::agent_id::A2aAgentId;
use a2a_nats::catalog::agent_view::{AgentViewCheckOutcome, SpiceDbSessionKey};
use a2a_nats::catalog::import_gate::{SpiceDbImportGateBuildError, SpiceDbPrincipal};
use a2a_nats::catalog::spicedb_permission::{AgentViewGate, NoopAgentViewGate, SpiceDbSessionCache};
use a2a_pack::resource_tuples::{
    Tier1A2aMethodSlug, Tier1DeriveError, Tier1DeriveInputs, Tier1ResourceId, Tier1ResourceTuple,
    Tier1ResourceTupleTable, Tier1ResourceType,
};
use async_trait::async_trait;
use tonic::Status;
use trogon_std::env::ReadEnv;

pub const ENV_TIER1_SPICEDB_ENABLED: &str = "A2A_GATEWAY_TIER1_SPICEDB_ENABLED";
pub const ENV_TIER1_SPICEDB_ENDPOINT: &str = "A2A_GATEWAY_TIER1_SPICEDB_ENDPOINT";
pub const ENV_TIER1_SPICEDB_TOKEN: &str = "A2A_GATEWAY_TIER1_SPICEDB_TOKEN";
pub const ENV_TIER1_ZEDTOKEN_TTL_SECS: &str = "A2A_GATEWAY_TIER1_ZEDTOKEN_TTL_SECS";

pub type Tier1SessionKey = SpiceDbSessionKey;
pub type SpiceDbTier1SessionCache = SpiceDbSessionCache;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Tier1SpiceDbBuildError {
    #[error("invalid tier-1 SpiceDB endpoint: {0}")]
    InvalidEndpoint(String),
    #[error("invalid tier-1 SpiceDB token: {0}")]
    InvalidToken(String),
    #[error("invalid tier-1 ZedToken TTL: {0}")]
    InvalidZedTokenTtl(String),
    #[error("tier-1 SpiceDB connect failed: {0}")]
    Connect(String),
    #[error("partial tier-1 SpiceDB configuration: {0}")]
    PartialConfig(String),
}

impl From<SpiceDbImportGateBuildError> for Tier1SpiceDbBuildError {
    fn from(error: SpiceDbImportGateBuildError) -> Self {
        match error {
            SpiceDbImportGateBuildError::InvalidEndpoint(msg) => Self::InvalidEndpoint(msg),
            SpiceDbImportGateBuildError::InvalidToken(msg) => Self::InvalidToken(msg),
            SpiceDbImportGateBuildError::InvalidZedTokenTtl(msg) => Self::InvalidZedTokenTtl(msg),
            SpiceDbImportGateBuildError::Connect(msg) => Self::Connect(msg),
        }
    }
}

/// Outcome a SpiceDB Tier-1 gate returns to the gateway's dispatch
/// path. `TransportError` is distinct from `Denied` so the audit
/// consumer can route "we couldn't ask" separately from "policy
/// said no", and `DeriveFailed` keeps payload-shape errors out of
/// the policy bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tier1AuthorizeOutcome {
    Allowed { zed_token: Option<String> },
    Denied,
    TransportError,
    DeriveFailed,
}

#[async_trait]
pub trait SpiceDbTier1Gate: Send + Sync {
    fn is_enabled(&self) -> bool;

    async fn authorize(
        &self,
        session: &Tier1SessionKey,
        principal: &SpiceDbPrincipal,
        tuple: &Tier1ResourceTuple,
    ) -> Tier1AuthorizeOutcome;

    async fn bulk_check_agent_view(
        &self,
        session: &Tier1SessionKey,
        principal: &SpiceDbPrincipal,
        agent_ids: &[A2aAgentId],
    ) -> Vec<AgentViewCheckOutcome>;
}

#[async_trait]
pub trait OwnerTupleEmitter: Send + Sync {
    async fn emit_owner(&self, owner: &Tier1OwnerTuple) -> Result<(), Status>;
}

/// Owner-relationship tuple the gateway writes back to SpiceDB
/// after a `tasks/create` so subsequent reads against the same
/// task can resolve owner-relation queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier1OwnerTuple {
    pub resource_type: Tier1ResourceType,
    pub resource_id: Tier1ResourceId,
    pub relation: String,
    pub subject_type: String,
    pub subject_id: String,
}

impl Tier1OwnerTuple {
    pub fn for_task(
        agent_id: &A2aAgentId,
        task_id: &str,
        subject_type: &str,
        subject_id: &str,
    ) -> Result<Self, Tier1DeriveError> {
        Ok(Self {
            resource_type: Tier1ResourceType::new("task")?,
            resource_id: Tier1ResourceId::new(format!("{}:{task_id}", agent_id.as_str()))?,
            relation: "owner".into(),
            subject_type: subject_type.to_owned(),
            subject_id: subject_id.to_owned(),
        })
    }
}

/// Resolve a Tier-1 resource tuple for a given A2A method, agent
/// id, and JSON-RPC params. Delegates to `a2a-pack`'s typed
/// `Tier1ResourceTupleTable` so the wire-shape parsing (which
/// JSON-RPC key holds the task id, etc.) stays at the
/// `Tier1DeriveInputs` boundary.
pub fn derive_tuple(
    method: &A2aMethod,
    agent_id: &A2aAgentId,
    publisher_account: &str,
    params: &serde_json::Value,
) -> Result<Tier1ResourceTuple, Tier1DeriveError> {
    let slug = Tier1A2aMethodSlug::new(method.as_str())?;
    let inputs = Tier1DeriveInputs::from_jsonrpc_params(slug, agent_id.as_str(), publisher_account, params);
    Tier1ResourceTupleTable::bundled().derive(&inputs)
}

/// Always-allow gate. Returns `Allowed { zed_token: None }` from
/// `authorize` and `Allowed` for every agent in
/// `bulk_check_agent_view`, mirroring [`NoopAgentViewGate`] for
/// the discovery side. `is_enabled = false` so callers can branch
/// on shadow-mode telemetry when an operator hasn't opted in.
#[derive(Debug, Default)]
pub struct NoopSpiceDbTier1Gate;

#[async_trait]
impl SpiceDbTier1Gate for NoopSpiceDbTier1Gate {
    fn is_enabled(&self) -> bool {
        false
    }

    async fn authorize(
        &self,
        _session: &Tier1SessionKey,
        _principal: &SpiceDbPrincipal,
        _tuple: &Tier1ResourceTuple,
    ) -> Tier1AuthorizeOutcome {
        Tier1AuthorizeOutcome::Allowed { zed_token: None }
    }

    async fn bulk_check_agent_view(
        &self,
        _session: &Tier1SessionKey,
        _principal: &SpiceDbPrincipal,
        agent_ids: &[A2aAgentId],
    ) -> Vec<AgentViewCheckOutcome> {
        vec![AgentViewCheckOutcome::Allowed; agent_ids.len()]
    }
}

pub struct GatewayTier1Layer {
    pub gate: Arc<dyn SpiceDbTier1Gate>,
    pub owner_emitter: Option<Arc<dyn OwnerTupleEmitter>>,
    pub discovery_view: Arc<dyn AgentViewGate>,
}

impl std::fmt::Debug for GatewayTier1Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayTier1Layer")
            .field("gate_is_enabled", &self.gate.is_enabled())
            .field("has_owner_emitter", &self.owner_emitter.is_some())
            .field("discovery_view_is_enabled", &self.discovery_view.is_enabled())
            .finish()
    }
}

impl GatewayTier1Layer {
    pub fn noop() -> Self {
        Self {
            gate: Arc::new(NoopSpiceDbTier1Gate),
            owner_emitter: None,
            discovery_view: Arc::new(NoopAgentViewGate),
        }
    }
}

pub struct Tier1SpiceDbConfig;

impl Tier1SpiceDbConfig {
    /// Resolve a [`GatewayTier1Layer`] from environment. Returns a
    /// Noop layer when `A2A_GATEWAY_TIER1_SPICEDB_ENABLED` is unset
    /// or off; fails closed when an operator enables Tier-1 without
    /// providing both endpoint and token. The Live gate itself
    /// ships in a follow-up slice -- this entry point intentionally
    /// errors when enabled to keep an operator from misreading
    /// "Noop returned silently" as "Tier-1 active".
    pub fn from_env<E: ReadEnv>(env: &E) -> Result<GatewayTier1Layer, Tier1SpiceDbBuildError> {
        // Always parse the TTL so an invalid value surfaces even
        // when Tier-1 isn't enabled -- catches a typo before it
        // becomes an outage at the moment the operator flips the
        // flag.
        let _ttl = tier1_zed_token_ttl(env)?;

        if !tier1_enabled(env) {
            return Ok(GatewayTier1Layer::noop());
        }

        // Sanity-check the credentials so a half-configured
        // deployment fails closed at boot rather than running on a
        // Noop gate operators believed was Live.
        let Some((_endpoint, _token)) = optional_tier1_credentials(env)? else {
            return Err(Tier1SpiceDbBuildError::PartialConfig(format!(
                "{ENV_TIER1_SPICEDB_ENABLED}=on requires {ENV_TIER1_SPICEDB_ENDPOINT} and {ENV_TIER1_SPICEDB_TOKEN}"
            )));
        };

        // `LiveSpiceDbTier1Gate` lives in the follow-up integration
        // PR alongside `LiveBulkImportPermissionClient`. Until that
        // lands, an operator with credentials configured still
        // gets the Noop gate; surface that explicitly so the audit
        // log shows the layer is shadow-only.
        Err(Tier1SpiceDbBuildError::Connect(
            "tier-1 live SpiceDB gate not yet wired; ships with the integration PR".into(),
        ))
    }
}

fn tier1_enabled<E: ReadEnv>(env: &E) -> bool {
    match env.var(ENV_TIER1_SPICEDB_ENABLED) {
        Ok(raw) => matches!(raw.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(_) => false,
    }
}

const DEFAULT_TIER1_ZEDTOKEN_TTL_SECS: u64 = 60;

fn tier1_zed_token_ttl<E: ReadEnv>(env: &E) -> Result<u64, Tier1SpiceDbBuildError> {
    match env.var(ENV_TIER1_ZEDTOKEN_TTL_SECS) {
        Ok(raw) => raw
            .parse::<u64>()
            .map_err(|_| Tier1SpiceDbBuildError::InvalidZedTokenTtl(raw)),
        Err(std::env::VarError::NotPresent) => Ok(DEFAULT_TIER1_ZEDTOKEN_TTL_SECS),
        Err(std::env::VarError::NotUnicode(_)) => Err(Tier1SpiceDbBuildError::InvalidZedTokenTtl(
            ENV_TIER1_ZEDTOKEN_TTL_SECS.into(),
        )),
    }
}

fn optional_tier1_credentials<E: ReadEnv>(env: &E) -> Result<Option<(String, String)>, Tier1SpiceDbBuildError> {
    let endpoint = match env.var(ENV_TIER1_SPICEDB_ENDPOINT) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(Tier1SpiceDbBuildError::InvalidEndpoint(
                ENV_TIER1_SPICEDB_ENDPOINT.into(),
            ));
        }
    };
    let token = match env.var(ENV_TIER1_SPICEDB_TOKEN) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(Tier1SpiceDbBuildError::InvalidToken(ENV_TIER1_SPICEDB_TOKEN.into()));
        }
    };

    match (endpoint, token) {
        (None, None) => Ok(None),
        (Some(endpoint), Some(token)) => Ok(Some((endpoint, token))),
        (Some(_), None) => Err(Tier1SpiceDbBuildError::PartialConfig(format!(
            "{ENV_TIER1_SPICEDB_TOKEN} is required when {ENV_TIER1_SPICEDB_ENDPOINT} is set"
        ))),
        (None, Some(_)) => Err(Tier1SpiceDbBuildError::PartialConfig(format!(
            "{ENV_TIER1_SPICEDB_ENDPOINT} is required when {ENV_TIER1_SPICEDB_TOKEN} is set"
        ))),
    }
}

#[cfg(test)]
mod tests;
