//! SpiceDB-backed agent-view permission gate + zed-token session
//! cache.
//!
//! Gated behind the `spicedb` Cargo feature so deployments that
//! don't run an external authoriser don't pay the
//! authzed/tonic/moka compile cost. The dep-free value objects
//! (`SpiceDbSessionKey`, `AgentViewCheckOutcome`, the discovery
//! filter outcomes, etc.) live in [`crate::catalog::agent_view`]
//! so callers that only need the typed surface can avoid this
//! feature entirely.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use authzed::v1::check_bulk_permissions_pair;
use authzed::v1::check_permission_response::Permissionship;
use authzed::v1::{
    CheckBulkPermissionsRequest, CheckBulkPermissionsRequestItem, Consistency, ObjectReference, SubjectReference,
    ZedToken,
};

use crate::agent_id::A2aAgentId;
use crate::catalog::agent_view::{AgentViewCheckOutcome, SpiceDbSessionKey};
use crate::catalog::import_gate::{
    BulkImportPermissionCheck, SpiceDbImportGateBuildError, SpiceDbPrincipal, ZedTokenSnapshot, ZedTokenTtl,
    spicedb_subject_from_principal as import_gate_spicedb_subject_from_principal,
};

pub const ENV_TIER1_SPICEDB_ENABLED: &str = "A2A_GATEWAY_TIER1_SPICEDB_ENABLED";
pub const ENV_TIER1_SPICEDB_ENDPOINT: &str = "A2A_GATEWAY_TIER1_SPICEDB_ENDPOINT";
pub const ENV_TIER1_SPICEDB_TOKEN: &str = "A2A_GATEWAY_TIER1_SPICEDB_TOKEN";
pub const ENV_TIER1_ZEDTOKEN_TTL_SECS: &str = "A2A_GATEWAY_TIER1_ZEDTOKEN_TTL_SECS";

const DEFAULT_TIER1_ZEDTOKEN_TTL_SECS: u64 = 60;
const SESSION_CACHE_CAPACITY: u64 = 4096;

/// Bounded LRU of fresh zed-token snapshots, keyed by the
/// `(principal, account)` pair. The cache lets the
/// `LiveAgentViewGate` switch the SpiceDB request from
/// `FullyConsistent` (slow, fresh) to `AtLeastAsFresh` (fast,
/// stale-tolerant) once we've seen any successful check for the
/// session -- TTL caps how stale "fresh enough" can drift.
#[derive(Clone)]
pub struct SpiceDbSessionCache {
    inner: moka::future::Cache<SpiceDbSessionKey, ZedTokenSnapshot>,
    ttl: ZedTokenTtl,
}

impl SpiceDbSessionCache {
    pub fn new(ttl: ZedTokenTtl) -> Self {
        Self {
            inner: moka::future::Cache::builder()
                .max_capacity(SESSION_CACHE_CAPACITY)
                .build(),
            ttl,
        }
    }

    pub async fn get(&self, key: &SpiceDbSessionKey) -> Option<ZedTokenSnapshot> {
        let snapshot = self.inner.get(key).await?;
        if snapshot.is_fresh(self.ttl.as_duration()) {
            Some(snapshot)
        } else {
            self.inner.invalidate(key).await;
            None
        }
    }

    pub async fn insert(&self, key: SpiceDbSessionKey, token: String) {
        self.inner
            .insert(
                key,
                ZedTokenSnapshot {
                    token,
                    observed_at: Instant::now(),
                },
            )
            .await;
    }
}

/// Trait abstraction over the agent-view permission check. The
/// gateway's discovery path consults a gate to decide which agents
/// to expose; production wires the [`LiveAgentViewGate`] backed by
/// SpiceDB, and tests / opt-out deployments use [`NoopAgentViewGate`]
/// which allows everything.
#[async_trait]
pub trait AgentViewGate: Send + Sync {
    fn is_enabled(&self) -> bool;

    async fn check_agent_view(
        &self,
        session: &SpiceDbSessionKey,
        principal: &SpiceDbPrincipal,
        agent_id: &A2aAgentId,
    ) -> AgentViewCheckOutcome;

    /// Default fan-out implementation issues one check per agent.
    /// [`LiveAgentViewGate`] overrides this to issue a single
    /// `CheckBulkPermissions` RPC, which is the round-trip-amortized
    /// fast path SpiceDB exposes for batch lookups.
    async fn bulk_check_agent_view(
        &self,
        session: &SpiceDbSessionKey,
        principal: &SpiceDbPrincipal,
        agent_ids: &[A2aAgentId],
    ) -> Vec<AgentViewCheckOutcome> {
        let mut outcomes = Vec::with_capacity(agent_ids.len());
        for agent_id in agent_ids {
            outcomes.push(self.check_agent_view(session, principal, agent_id).await);
        }
        outcomes
    }
}

/// Always-allow gate for deployments without an authoriser (or
/// tests). `is_enabled() = false` so callers can branch on the
/// shadow path (audit-only) when an operator hasn't opted in.
#[derive(Debug, Default)]
pub struct NoopAgentViewGate;

#[async_trait]
impl AgentViewGate for NoopAgentViewGate {
    fn is_enabled(&self) -> bool {
        false
    }

    async fn check_agent_view(
        &self,
        _session: &SpiceDbSessionKey,
        _principal: &SpiceDbPrincipal,
        _agent_id: &A2aAgentId,
    ) -> AgentViewCheckOutcome {
        AgentViewCheckOutcome::Allowed
    }
}

/// SpiceDB-backed gate. Wraps the workspace-shared
/// [`BulkImportPermissionCheck`] client (so unit tests can swap in
/// a fake) plus a [`SpiceDbSessionCache`] used to upgrade
/// per-session consistency from `FullyConsistent` to
/// `AtLeastAsFresh` after the first successful response.
pub struct LiveAgentViewGate {
    client: Arc<dyn BulkImportPermissionCheck>,
    session_cache: SpiceDbSessionCache,
}

impl LiveAgentViewGate {
    pub fn new(client: Arc<dyn BulkImportPermissionCheck>, session_cache: SpiceDbSessionCache) -> Self {
        Self { client, session_cache }
    }
}

#[async_trait]
impl AgentViewGate for LiveAgentViewGate {
    fn is_enabled(&self) -> bool {
        true
    }

    async fn check_agent_view(
        &self,
        session: &SpiceDbSessionKey,
        principal: &SpiceDbPrincipal,
        agent_id: &A2aAgentId,
    ) -> AgentViewCheckOutcome {
        let outcomes = self
            .bulk_check_agent_view(session, principal, std::slice::from_ref(agent_id))
            .await;
        outcomes
            .into_iter()
            .next()
            .unwrap_or(AgentViewCheckOutcome::TransportError)
    }

    async fn bulk_check_agent_view(
        &self,
        session: &SpiceDbSessionKey,
        principal: &SpiceDbPrincipal,
        agent_ids: &[A2aAgentId],
    ) -> Vec<AgentViewCheckOutcome> {
        if agent_ids.is_empty() {
            return Vec::new();
        }

        // No `spicedb_subject` claim on the principal means we
        // can't even build a SubjectReference -- fail closed (Denied
        // per agent) rather than letting the request through.
        let Some((subject_type, subject_id)) = spicedb_subject_from_principal(principal) else {
            return vec![AgentViewCheckOutcome::Denied; agent_ids.len()];
        };

        let items: Vec<CheckBulkPermissionsRequestItem> = agent_ids
            .iter()
            .map(|agent_id| CheckBulkPermissionsRequestItem {
                resource: Some(ObjectReference {
                    object_type: "agent".into(),
                    object_id: agent_id.as_str().to_owned(),
                }),
                permission: "view".into(),
                subject: Some(SubjectReference {
                    object: Some(ObjectReference {
                        object_type: subject_type.clone(),
                        object_id: subject_id.clone(),
                    }),
                    optional_relation: String::new(),
                }),
                context: None,
            })
            .collect();

        let mut request = CheckBulkPermissionsRequest {
            items,
            consistency: None,
            with_tracing: false,
        };

        // First request after a session cache miss / TTL expiry
        // pays the `FullyConsistent` cost so we get back a fresh
        // zed-token; subsequent requests inside the TTL window can
        // use the cheaper `AtLeastAsFresh` mode.
        if let Some(snapshot) = self.session_cache.get(session).await {
            request.consistency = Some(Consistency {
                requirement: Some(authzed::v1::consistency::Requirement::AtLeastAsFresh(ZedToken {
                    token: snapshot.token,
                })),
            });
        } else {
            request.consistency = Some(Consistency {
                requirement: Some(authzed::v1::consistency::Requirement::FullyConsistent(true)),
            });
        }

        let response = match self.client.check_bulk_permissions(request).await {
            Ok(response) => response,
            // Transport-level failures (timeouts, RPC errors) must
            // surface distinct from Denied so the audit consumer
            // can route them to the "couldn't ask" bucket.
            Err(_) => return vec![AgentViewCheckOutcome::TransportError; agent_ids.len()],
        };

        if let Some(token) = response.checked_at.map(|zed| zed.token) {
            self.session_cache.insert(session.clone(), token).await;
        }

        // The SpiceDB bulk RPC must return exactly one pair per
        // requested item -- a shorter response is a protocol
        // violation. `zip` would silently truncate, leaving a
        // shorter outcome vec than `agent_ids` and tripping the
        // single-agent helper into `TransportError` for items the
        // RPC actually answered.  Fail closed instead: pad with
        // `TransportError` so the per-agent contract holds and the
        // audit consumer sees the protocol mismatch as a transport
        // bucket, not silently labelled `Allowed`.
        let mut outcomes = Vec::with_capacity(agent_ids.len());
        for (idx, _) in agent_ids.iter().enumerate() {
            let outcome = match response.pairs.get(idx) {
                Some(pair) if pair_is_allowed(pair) => AgentViewCheckOutcome::Allowed,
                Some(_) => AgentViewCheckOutcome::Denied,
                None => AgentViewCheckOutcome::TransportError,
            };
            outcomes.push(outcome);
        }
        outcomes
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

/// Extract `(object_type, object_id)` from the principal's
/// `spicedb_subject` claim. Delegates to the import-gate parser so
/// the Tier-1 and import-gate paths agree on the subject grammar.
pub fn spicedb_subject_from_principal(principal: &SpiceDbPrincipal) -> Option<(String, String)> {
    import_gate_spicedb_subject_from_principal(principal)
}

pub fn tier1_enabled<E: trogon_std::env::ReadEnv>(env: &E) -> bool {
    match env.var(ENV_TIER1_SPICEDB_ENABLED) {
        Ok(raw) => matches!(raw.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(_) => false,
    }
}

pub fn tier1_zed_token_ttl<E: trogon_std::env::ReadEnv>(env: &E) -> Result<ZedTokenTtl, SpiceDbImportGateBuildError> {
    match env.var(ENV_TIER1_ZEDTOKEN_TTL_SECS) {
        Ok(raw) => raw
            .parse::<u64>()
            .map(ZedTokenTtl::from_secs)
            .map_err(|_| SpiceDbImportGateBuildError::InvalidZedTokenTtl(raw)),
        Err(std::env::VarError::NotPresent) => Ok(ZedTokenTtl::from_secs(DEFAULT_TIER1_ZEDTOKEN_TTL_SECS)),
        Err(std::env::VarError::NotUnicode(_)) => Err(SpiceDbImportGateBuildError::InvalidZedTokenTtl(
            ENV_TIER1_ZEDTOKEN_TTL_SECS.into(),
        )),
    }
}

/// Env-resolved layer that picks between [`NoopAgentViewGate`] (no
/// auth) and a caller-supplied [`LiveAgentViewGate`] (SpiceDB)
/// based on `A2A_GATEWAY_TIER1_SPICEDB_ENABLED`. Connecting the
/// live gRPC client is the caller's responsibility -- this slice
/// keeps the live-client wiring out of `a2a-nats` so the gateway's
/// integration PR can land its smoke harness against a real
/// SpiceDB server in one place.
pub struct AgentViewGateLayer {
    pub gate: Arc<dyn AgentViewGate>,
}

impl AgentViewGateLayer {
    pub fn noop() -> Self {
        Self {
            gate: Arc::new(NoopAgentViewGate),
        }
    }

    pub fn with_gate(gate: Arc<dyn AgentViewGate>) -> Self {
        Self { gate }
    }
}

/// Build a [`SpiceDbSessionKey`] from a verified principal,
/// falling back to the supplied account when the principal didn't
/// carry an explicit `session_account` / `account` / `aud` claim.
pub fn session_from_principal(principal: &SpiceDbPrincipal, fallback_account: &str) -> Option<SpiceDbSessionKey> {
    let sub = principal
        .0
        .get("sub")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            principal
                .0
                .get("spicedb_subject")
                .and_then(serde_json::Value::as_str)
                .and_then(parse_subject_id)
        })?;

    let account = principal
        .0
        .get("session_account")
        .or_else(|| principal.0.get("account"))
        .or_else(|| principal.0.get("aud"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback_account);

    Some(SpiceDbSessionKey::new(sub, account))
}

/// Strip the SpiceDB type prefix from a subject reference so the
/// session key carries just the principal id. Tolerates both
/// `agent/x` and `agent:x` separator shapes -- different identity
/// providers emit different forms. A separator present with an
/// empty right-hand side (e.g. `"user/"`) returns `None` rather
/// than the malformed raw string, so a session key built from this
/// value can't carry a placeholder id.
fn parse_subject_id(raw: &str) -> Option<String> {
    for sep in ['/', ':'] {
        if let Some((object_type, object_id)) = raw.split_once(sep) {
            if object_id.is_empty() {
                return None;
            }
            let _ = object_type;
            return Some(object_id.to_owned());
        }
    }
    if raw.is_empty() { None } else { Some(raw.to_owned()) }
}

#[cfg(test)]
mod tests;
