//! SpiceDB-backed Tier-1 authorization gate.
//!
//! Gated behind the `spicedb` feature so deployments without an
//! external authoriser don't pay the authzed/tonic compile cost.
//! Ships the typed gate trait, Noop + Live impls, the outcome
//! enum, the audit-side owner-tuple emitter, and the env-driven
//! layer config that constructs either a Noop (gate disabled) or
//! a tonic-backed Live gate (gate enabled with credentials).

use std::sync::Arc;

use a2a_nats::A2aMethod;
use a2a_nats::agent_id::A2aAgentId;
use a2a_nats::catalog::agent_view::{AgentViewCheckOutcome, SpiceDbSessionKey};
use a2a_nats::catalog::import_gate::{
    BulkImportPermissionCheck, SpiceDbEndpoint, SpiceDbImportGateBuildError, SpiceDbPrincipal, SpiceDbToken,
    ZedTokenTtl, spicedb_subject_from_principal,
};
use a2a_nats::catalog::spicedb_permission::{AgentViewGate, LiveAgentViewGate, NoopAgentViewGate, SpiceDbSessionCache};
use a2a_pack::resource_tuples::{
    Tier1A2aMethodSlug, Tier1DeriveError, Tier1DeriveInputs, Tier1ResourceId, Tier1ResourceTuple,
    Tier1ResourceTupleTable, Tier1ResourceType,
};
use async_trait::async_trait;
use authzed::v1::check_bulk_permissions_pair;
use authzed::v1::check_permission_response::Permissionship;
use authzed::v1::relationship_update::Operation;
use authzed::v1::{
    CheckBulkPermissionsRequest, CheckBulkPermissionsRequestItem, Consistency, ObjectReference, Relationship,
    RelationshipUpdate, SubjectReference, WriteRelationshipsRequest, ZedToken,
};
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
    #[error("{enabled_var}=on requires both {endpoint_var} and {token_var} (missing: {missing_var})")]
    EnabledMissingCredentials {
        enabled_var: &'static str,
        endpoint_var: &'static str,
        token_var: &'static str,
        missing_var: &'static str,
    },
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

/// SpiceDB-backed Tier-1 gate. Issues `CheckBulkPermissions` RPCs
/// to authorize requests against a single tuple; reuses the same
/// session cache as `LiveAgentViewGate` so the per-session
/// consistency upgrade (`FullyConsistent` -> `AtLeastAsFresh`)
/// applies across both surfaces.
pub struct LiveSpiceDbTier1Gate {
    client: Arc<dyn BulkImportPermissionCheck>,
    session_cache: SpiceDbTier1SessionCache,
    view_gate: LiveAgentViewGate,
}

impl std::fmt::Debug for LiveSpiceDbTier1Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveSpiceDbTier1Gate").finish_non_exhaustive()
    }
}

impl LiveSpiceDbTier1Gate {
    pub fn new(client: Arc<dyn BulkImportPermissionCheck>, session_cache: SpiceDbTier1SessionCache) -> Self {
        let view_gate = LiveAgentViewGate::new(client.clone(), session_cache.clone());
        Self {
            client,
            session_cache,
            view_gate,
        }
    }
}

#[async_trait]
impl OwnerTupleEmitter for LiveSpiceDbTier1Gate {
    async fn emit_owner(&self, owner: &Tier1OwnerTuple) -> Result<(), Status> {
        let request = WriteRelationshipsRequest {
            updates: vec![RelationshipUpdate {
                operation: Operation::Touch as i32,
                relationship: Some(Relationship {
                    resource: Some(ObjectReference {
                        object_type: owner.resource_type.as_str().to_owned(),
                        object_id: owner.resource_id.as_str().to_owned(),
                    }),
                    relation: owner.relation.clone(),
                    subject: Some(SubjectReference {
                        object: Some(ObjectReference {
                            object_type: owner.subject_type.clone(),
                            object_id: owner.subject_id.clone(),
                        }),
                        optional_relation: String::new(),
                    }),
                    optional_caveat: None,
                    optional_expires_at: None,
                }),
            }],
            optional_preconditions: Vec::new(),
            optional_transaction_metadata: None,
        };
        self.client.write_relationships(request).await.map(|_| ())
    }
}

#[async_trait]
impl SpiceDbTier1Gate for LiveSpiceDbTier1Gate {
    fn is_enabled(&self) -> bool {
        true
    }

    async fn authorize(
        &self,
        session: &Tier1SessionKey,
        principal: &SpiceDbPrincipal,
        tuple: &Tier1ResourceTuple,
    ) -> Tier1AuthorizeOutcome {
        // No subject claim -> fail closed. The session log makes
        // the denial cause visible without leaking the principal
        // payload itself.
        let Some((subject_type, subject_id)) = spicedb_tier1_subject_from_principal(principal) else {
            tracing::warn!(
                session_sub = %session.sub(),
                session_account = %session.account(),
                "SpiceDbTier1Gate denying: principal lacks subject mapping",
            );
            return Tier1AuthorizeOutcome::Denied;
        };

        let mut request = CheckBulkPermissionsRequest {
            items: vec![CheckBulkPermissionsRequestItem {
                resource: Some(ObjectReference {
                    object_type: tuple.resource_type.as_str().to_owned(),
                    object_id: tuple.resource_id.as_str().to_owned(),
                }),
                permission: tuple.permission.as_str().to_owned(),
                subject: Some(SubjectReference {
                    object: Some(ObjectReference {
                        object_type: subject_type,
                        object_id: subject_id,
                    }),
                    optional_relation: String::new(),
                }),
                context: None,
            }],
            consistency: None,
            with_tracing: false,
        };

        // First request after a session cache miss pays the
        // `FullyConsistent` round-trip cost so we get a fresh
        // zed-token; subsequent requests inside the TTL window
        // use the cheaper `AtLeastAsFresh` mode.
        request.consistency = Some(if let Some(snapshot) = self.session_cache.get(session).await {
            Consistency {
                requirement: Some(authzed::v1::consistency::Requirement::AtLeastAsFresh(ZedToken {
                    token: snapshot.token,
                })),
            }
        } else {
            Consistency {
                requirement: Some(authzed::v1::consistency::Requirement::FullyConsistent(true)),
            }
        });

        let response = match self.client.check_bulk_permissions(request).await {
            Ok(response) => response.into_inner(),
            Err(error) => {
                tracing::warn!(
                    session_sub = %session.sub(),
                    resource = %tuple.resource_id.as_str(),
                    error = %error,
                    "SpiceDbTier1Gate transport error",
                );
                return Tier1AuthorizeOutcome::TransportError;
            }
        };

        // Cache the zed-token before branching on allow/deny: a
        // successful SpiceDB response carries `checked_at` even when
        // the permissionship is `NoPermission`, and reusing that
        // snapshot lets subsequent checks for the same session
        // (including the shared `LiveAgentViewGate`) skip the
        // `FullyConsistent` round-trip.
        let zed_token = response.checked_at.map(|zed| zed.token);
        if let Some(token) = zed_token.clone() {
            self.session_cache.insert(session.clone(), token).await;
        }

        let allowed = response.pairs.first().is_some_and(pair_is_allowed);
        if !allowed {
            return Tier1AuthorizeOutcome::Denied;
        }

        Tier1AuthorizeOutcome::Allowed { zed_token }
    }

    async fn bulk_check_agent_view(
        &self,
        session: &Tier1SessionKey,
        principal: &SpiceDbPrincipal,
        agent_ids: &[A2aAgentId],
    ) -> Vec<AgentViewCheckOutcome> {
        self.view_gate
            .bulk_check_agent_view(session, principal, agent_ids)
            .await
    }
}

fn pair_is_allowed(pair: &authzed::v1::CheckBulkPermissionsPair) -> bool {
    match pair.response.as_ref() {
        Some(check_bulk_permissions_pair::Response::Item(item)) => {
            item.permissionship == Permissionship::HasPermission as i32
        }
        Some(check_bulk_permissions_pair::Response::Error(_)) | None => false,
    }
}

/// Extract `(object_type, object_id)` from a principal, delegating
/// to the import-gate resolver so the Tier-1 authorize path and
/// the discovery-side `bulk_check_agent_view` path (which goes
/// through `LiveAgentViewGate`) agree on the same precedence
/// (`account`/`aud` before `spicedb_subject`). Without that, the
/// same principal could be authorized under one subject and
/// filtered under another -- a silent inconsistency we'd never
/// see in unit tests.
fn spicedb_tier1_subject_from_principal(principal: &SpiceDbPrincipal) -> Option<(String, String)> {
    spicedb_subject_from_principal(principal)
}

/// Build a Tier-1 session key from a verified principal, falling
/// back to the supplied account when the principal didn't carry
/// an explicit account claim. Delegates to the a2a-nats helper so
/// the Tier-1 and discovery paths agree on the key shape.
pub fn tier1_session_from_principal(principal: &SpiceDbPrincipal, fallback_account: &str) -> Option<Tier1SessionKey> {
    a2a_nats::catalog::spicedb_permission::session_from_principal(principal, fallback_account)
}

/// Build a principal payload from a caller slug + session account.
/// Used by the gateway runtime to materialise a principal from the caller
/// identity dispatch resolved for this request -- the JWT-header identity by
/// default, or the verified `aa-auth+jwt` principal when
/// `gateway_caller_identity_after_aauth` (in `runtime::aauth_env`) determined
/// one supersedes it.
pub fn tier1_principal_from_caller(caller_slug: &str, account: &str) -> SpiceDbPrincipal {
    SpiceDbPrincipal(serde_json::json!({
        "spicedb_subject": format!("user/{caller_slug}"),
        "sub": caller_slug,
        "session_account": account,
    }))
}

pub struct Tier1SpiceDbConfig;

impl Tier1SpiceDbConfig {
    /// Resolve a [`GatewayTier1Layer`] from environment.
    ///
    /// Returns a Noop layer when `A2A_GATEWAY_TIER1_SPICEDB_ENABLED`
    /// is unset or off; constructs a Live gate (tonic-backed
    /// SpiceDB client + session cache + agent-view fanout) when
    /// enabled. Fails closed when an operator enables Tier-1
    /// without providing both endpoint and token, and surfaces
    /// transport errors at boot rather than at first request so
    /// half-configured deployments never silently run on Noop.
    pub async fn from_env<E: ReadEnv>(env: &E) -> Result<GatewayTier1Layer, Tier1SpiceDbBuildError> {
        let ttl_secs = tier1_zed_token_ttl(env)?;

        if !tier1_enabled(env) {
            return Ok(GatewayTier1Layer::noop());
        }

        let Some((endpoint, token)) = optional_tier1_credentials(env)? else {
            return Err(Tier1SpiceDbBuildError::EnabledMissingCredentials {
                enabled_var: ENV_TIER1_SPICEDB_ENABLED,
                endpoint_var: ENV_TIER1_SPICEDB_ENDPOINT,
                token_var: ENV_TIER1_SPICEDB_TOKEN,
                missing_var: "both",
            });
        };

        let endpoint = SpiceDbEndpoint::parse(endpoint)?;
        let token = SpiceDbToken::parse(token)?;
        let ttl = ZedTokenTtl::from_secs(ttl_secs);

        let client =
            Arc::new(a2a_nats::catalog::import_gate::LiveBulkImportPermissionClient::connect(&endpoint, &token).await?);
        let session_cache = SpiceDbTier1SessionCache::new(ttl);
        let live = Arc::new(LiveSpiceDbTier1Gate::new(client.clone(), session_cache.clone()));
        let discovery_view = Arc::new(LiveAgentViewGate::new(client, session_cache));

        Ok(GatewayTier1Layer {
            gate: live.clone(),
            owner_emitter: Some(live),
            discovery_view,
        })
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
        (Some(_), None) => Err(Tier1SpiceDbBuildError::EnabledMissingCredentials {
            enabled_var: ENV_TIER1_SPICEDB_ENABLED,
            endpoint_var: ENV_TIER1_SPICEDB_ENDPOINT,
            token_var: ENV_TIER1_SPICEDB_TOKEN,
            missing_var: ENV_TIER1_SPICEDB_TOKEN,
        }),
        (None, Some(_)) => Err(Tier1SpiceDbBuildError::EnabledMissingCredentials {
            enabled_var: ENV_TIER1_SPICEDB_ENABLED,
            endpoint_var: ENV_TIER1_SPICEDB_ENDPOINT,
            token_var: ENV_TIER1_SPICEDB_TOKEN,
            missing_var: ENV_TIER1_SPICEDB_ENDPOINT,
        }),
    }
}

#[cfg(test)]
mod tests;
