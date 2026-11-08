//! SpiceDB-backed Tier-1 authorization gate.
//!
//! Gated behind the `spicedb` feature so deployments without an
//! external authoriser don't pay the authzed/tonic compile cost.
//! Ships the tonic-backed Live gate and the env-driven layer config
//! that constructs either a Noop (gate disabled) or a Live gate (gate
//! enabled with credentials). The gate trait, outcome, Noop gate and
//! owner-tuple emitter trait live ungated in
//! [`crate::policy::tier1_gate`] and are re-exported here so existing
//! paths keep compiling.

use std::sync::Arc;

use a2a_nats::agent_id::A2aAgentId;
use a2a_nats::catalog::agent_view::AgentViewCheckOutcome;
use a2a_nats::catalog::import_gate::{
    BulkImportPermissionCheck, SpiceDbEndpoint, SpiceDbImportGateBuildError, SpiceDbPrincipal, SpiceDbToken,
    ZedTokenTtl, spicedb_subject_from_principal,
};
use a2a_nats::catalog::spicedb_permission::{AgentViewGate, LiveAgentViewGate, SpiceDbSessionCache};
use a2a_pack::resource_tuples::Tier1ResourceTuple;
use async_trait::async_trait;
use authzed::v1::check_bulk_permissions_pair;
use authzed::v1::check_permission_response::Permissionship;
use authzed::v1::relationship_update::Operation;
use authzed::v1::{
    CheckBulkPermissionsRequest, CheckBulkPermissionsRequestItem, Consistency, ObjectReference, Relationship,
    RelationshipUpdate, SubjectReference, WriteRelationshipsRequest, ZedToken,
};
use trogon_std::env::ReadEnv;

use crate::constants::DEFAULT_TIER1_ZEDTOKEN_TTL_SECS;
pub use crate::constants::{
    ENV_TIER1_SPICEDB_ENABLED, ENV_TIER1_SPICEDB_ENDPOINT, ENV_TIER1_SPICEDB_TOKEN, ENV_TIER1_ZEDTOKEN_TTL_SECS,
};
use crate::policy::tier1_gate::tier1_enabled;
pub use crate::policy::tier1_gate::{
    GatewayTier1Layer, NoopSpiceDbTier1Gate, OwnerEmitError, OwnerTupleEmitter, SpiceDbTier1Gate,
    Tier1AuthorizeOutcome, Tier1OwnerTuple, Tier1SessionKey, Tier1SpiceDbBuildError, Tier1SpiceDbConfig, derive_tuple,
    tier1_principal_from_caller, tier1_session_from_caller,
};

pub type SpiceDbTier1SessionCache = SpiceDbSessionCache;

impl From<tonic::Status> for OwnerEmitError {
    fn from(status: tonic::Status) -> Self {
        Self::new(status)
    }
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
    async fn emit_owner(&self, owner: &Tier1OwnerTuple) -> Result<(), OwnerEmitError> {
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
        self.client
            .write_relationships(request)
            .await
            .map(|_| ())
            .map_err(OwnerEmitError::from)
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
