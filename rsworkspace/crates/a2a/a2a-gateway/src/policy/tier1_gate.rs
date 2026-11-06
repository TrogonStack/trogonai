//! Tier-1 relational-authorization seam.
//!
//! The gate trait, its outcome, the owner-tuple emitter, and the
//! always-allow [`NoopSpiceDbTier1Gate`] carry no authzed/tonic
//! dependency, so dispatch runs the same code path in every build.
//! Only the Live SpiceDB gate and its env-driven config live behind
//! the `spicedb` feature in [`crate::policy::spicedb_tier1`]; a build
//! without the feature resolves Tier-1 to the Noop layer and refuses to
//! start when an operator asks for the Live gate.

use std::sync::Arc;

use a2a_nats::A2aMethod;
use a2a_nats::agent_id::A2aAgentId;
use a2a_nats::catalog::agent_view::{AgentViewCheckOutcome, SpiceDbSessionKey};
use a2a_nats::catalog::import_gate::SpiceDbPrincipal;
use a2a_pack::resource_tuples::{
    Tier1A2aMethodSlug, Tier1DeriveError, Tier1DeriveInputs, Tier1ResourceId, Tier1ResourceTuple,
    Tier1ResourceTupleTable, Tier1ResourceType,
};
use async_trait::async_trait;
use trogon_identity_types::SubjectPrincipal;
use trogon_std::env::ReadEnv;

use crate::constants::ENV_TIER1_SPICEDB_ENABLED;
use crate::policy::CallerSpiceDbSubject;

pub type Tier1SessionKey = SpiceDbSessionKey;

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
    #[error("{enabled_var}=on requires both {endpoint_var} and {token_var} (missing: {missing_var})")]
    EnabledMissingCredentials {
        enabled_var: &'static str,
        endpoint_var: &'static str,
        token_var: &'static str,
        missing_var: &'static str,
    },
    #[error("{enabled_var} is on but this build was compiled without the `spicedb` feature")]
    NotCompiledIn { enabled_var: &'static str },
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

/// Transport-neutral failure writing an owner tuple. Carries the
/// backend's message so the dispatch warning stays diagnosable
/// without the trait naming a gRPC type.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("owner tuple emit failed: {0}")]
pub struct OwnerEmitError(String);

impl OwnerEmitError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

#[async_trait]
pub trait OwnerTupleEmitter: Send + Sync {
    async fn emit_owner(&self, owner: &Tier1OwnerTuple) -> Result<(), OwnerEmitError>;
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
/// `bulk_check_agent_view`, mirroring `NoopAgentViewGate` for
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
    #[cfg(feature = "spicedb")]
    pub discovery_view: Arc<dyn a2a_nats::catalog::spicedb_permission::AgentViewGate>,
}

impl std::fmt::Debug for GatewayTier1Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("GatewayTier1Layer");
        debug
            .field("gate_is_enabled", &self.gate.is_enabled())
            .field("has_owner_emitter", &self.owner_emitter.is_some());
        #[cfg(feature = "spicedb")]
        debug.field("discovery_view_is_enabled", &self.discovery_view.is_enabled());
        debug.finish()
    }
}

impl GatewayTier1Layer {
    pub fn noop() -> Self {
        Self {
            gate: Arc::new(NoopSpiceDbTier1Gate),
            owner_emitter: None,
            #[cfg(feature = "spicedb")]
            discovery_view: Arc::new(a2a_nats::catalog::spicedb_permission::NoopAgentViewGate),
        }
    }
}

pub fn tier1_principal_from_caller(caller: &CallerSpiceDbSubject) -> SpiceDbPrincipal {
    SpiceDbPrincipal::new(caller.to_string())
}

pub fn tier1_session_from_caller(caller: &SubjectPrincipal, account: &str) -> Tier1SessionKey {
    Tier1SessionKey::new(caller.to_string(), account)
}

pub(crate) fn tier1_enabled<E: ReadEnv>(env: &E) -> bool {
    match env.var(ENV_TIER1_SPICEDB_ENABLED) {
        Ok(raw) => matches!(raw.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(_) => false,
    }
}

/// Env-driven Tier-1 SpiceDB layer resolver. The Live branch is
/// implemented in [`crate::policy::spicedb_tier1`] under the
/// `spicedb` feature; the default build's implementation below
/// yields the Noop layer and refuses to boot when the operator
/// turned the gate on.
pub struct Tier1SpiceDbConfig;

#[cfg(not(feature = "spicedb"))]
impl Tier1SpiceDbConfig {
    /// A deployment that asked for SpiceDB enforcement must not run
    /// unenforced because the binary was built without it, so an
    /// enabled gate is a startup error rather than a silent Noop.
    pub async fn from_env<E: ReadEnv>(env: &E) -> Result<GatewayTier1Layer, Tier1SpiceDbBuildError> {
        if tier1_enabled(env) {
            return Err(Tier1SpiceDbBuildError::NotCompiledIn {
                enabled_var: ENV_TIER1_SPICEDB_ENABLED,
            });
        }
        Ok(GatewayTier1Layer::noop())
    }
}

#[cfg(test)]
mod tests;
