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
use crate::catalog::import_gate::{
    BulkImportPermissionCheck, SpiceDbEndpoint, SpiceDbImportGateBuildError, SpiceDbPrincipal, SpiceDbToken,
    ZedTokenSnapshot, ZedTokenTtl, parse_subject_reference,
};

pub const ENV_TIER1_SPICEDB_ENABLED: &str = "A2A_GATEWAY_TIER1_SPICEDB_ENABLED";
pub const ENV_TIER1_SPICEDB_ENDPOINT: &str = "A2A_GATEWAY_TIER1_SPICEDB_ENDPOINT";
pub const ENV_TIER1_SPICEDB_TOKEN: &str = "A2A_GATEWAY_TIER1_SPICEDB_TOKEN";
pub const ENV_TIER1_ZEDTOKEN_TTL_SECS: &str = "A2A_GATEWAY_TIER1_ZEDTOKEN_TTL_SECS";

const DEFAULT_TIER1_ZEDTOKEN_TTL_SECS: u64 = 60;
const SESSION_CACHE_CAPACITY: u64 = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SpiceDbSessionKey {
    pub sub: String,
    pub account: String,
}

impl SpiceDbSessionKey {
    pub fn new(sub: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            sub: sub.into(),
            account: account.into(),
        }
    }

    pub fn sub(&self) -> &str {
        &self.sub
    }

    pub fn account(&self) -> &str {
        &self.account
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentViewTuple {
    pub agent_id: A2aAgentId,
}

impl AgentViewTuple {
    pub fn resource_id(&self) -> &str {
        self.agent_id.as_str()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentViewCheckOutcome {
    Allowed,
    Denied,
    TransportError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryAgentFilterOutcome {
    Visible(A2aAgentId),
    Hidden {
        agent_id: A2aAgentId,
        reason: DiscoveryHiddenReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryHiddenReason {
    Denied,
    TransportError,
}

pub fn filter_agents_by_view(
    agent_ids: &[A2aAgentId],
    outcomes: &[AgentViewCheckOutcome],
) -> Vec<DiscoveryAgentFilterOutcome> {
    agent_ids
        .iter()
        .zip(outcomes.iter())
        .map(|(agent_id, outcome)| match outcome {
            AgentViewCheckOutcome::Allowed => DiscoveryAgentFilterOutcome::Visible(agent_id.clone()),
            AgentViewCheckOutcome::Denied => DiscoveryAgentFilterOutcome::Hidden {
                agent_id: agent_id.clone(),
                reason: DiscoveryHiddenReason::Denied,
            },
            AgentViewCheckOutcome::TransportError => DiscoveryAgentFilterOutcome::Hidden {
                agent_id: agent_id.clone(),
                reason: DiscoveryHiddenReason::TransportError,
            },
        })
        .collect()
}

#[derive(Clone)]
pub struct SpiceDbSessionCache {
    inner: moka::future::Cache<SpiceDbSessionKey, ZedTokenSnapshot>,
    ttl: ZedTokenTtl,
}

impl SpiceDbSessionCache {
    pub fn new(ttl: ZedTokenTtl) -> Self {
        Self {
            inner: moka::future::Cache::builder().max_capacity(SESSION_CACHE_CAPACITY).build(),
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

#[async_trait]
pub trait AgentViewGate: Send + Sync {
    fn is_enabled(&self) -> bool;

    async fn check_agent_view(
        &self,
        session: &SpiceDbSessionKey,
        principal: &SpiceDbPrincipal,
        agent_id: &A2aAgentId,
    ) -> AgentViewCheckOutcome;

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

pub struct LiveAgentViewGate {
    client: Arc<dyn BulkImportPermissionCheck>,
    session_cache: SpiceDbSessionCache,
}

impl LiveAgentViewGate {
    pub fn new(client: Arc<dyn BulkImportPermissionCheck>, session_cache: SpiceDbSessionCache) -> Self {
        Self {
            client,
            session_cache,
        }
    }

    pub async fn connect(
        endpoint: &SpiceDbEndpoint,
        token: &SpiceDbToken,
        ttl: ZedTokenTtl,
    ) -> Result<Self, SpiceDbImportGateBuildError> {
        let client = crate::catalog::import_gate::LiveBulkImportPermissionClient::connect(endpoint, token).await?;
        Ok(Self::new(Arc::new(client), SpiceDbSessionCache::new(ttl)))
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
        outcomes.into_iter().next().unwrap_or(AgentViewCheckOutcome::TransportError)
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

        if let Some(snapshot) = self.session_cache.get(session).await {
            request.consistency = Some(Consistency {
                requirement: Some(authzed::v1::consistency::Requirement::AtLeastAsFresh(
                    ZedToken {
                        token: snapshot.token,
                    },
                )),
            });
        } else {
            request.consistency = Some(Consistency {
                requirement: Some(authzed::v1::consistency::Requirement::FullyConsistent(true)),
            });
        }

        let response = match self.client.check_bulk_permissions(request).await {
            Ok(response) => response,
            Err(_) => return vec![AgentViewCheckOutcome::TransportError; agent_ids.len()],
        };

        if let Some(token) = response.checked_at.map(|zed| zed.token) {
            self.session_cache.insert(session.clone(), token).await;
        }

        agent_ids
            .iter()
            .zip(response.pairs.iter())
            .map(|(_agent_id, pair)| {
                if pair_is_allowed(pair) {
                    AgentViewCheckOutcome::Allowed
                } else {
                    AgentViewCheckOutcome::Denied
                }
            })
            .collect()
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

pub fn spicedb_subject_from_principal(principal: &SpiceDbPrincipal) -> Option<(String, String)> {
    principal
        .0
        .get("spicedb_subject")
        .and_then(serde_json::Value::as_str)
        .and_then(parse_subject_reference)
}

pub fn tier1_enabled<E: trogon_std::env::ReadEnv>(env: &E) -> bool {
    match env.var(ENV_TIER1_SPICEDB_ENABLED) {
        Ok(raw) => matches!(raw.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(std::env::VarError::NotPresent) => false,
        Err(std::env::VarError::NotUnicode(_)) => false,
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

fn optional_tier1_credentials<E: trogon_std::env::ReadEnv>(
    env: &E,
) -> Result<Option<(SpiceDbEndpoint, SpiceDbToken)>, SpiceDbImportGateBuildError> {
    let endpoint = match env.var(ENV_TIER1_SPICEDB_ENDPOINT) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(SpiceDbImportGateBuildError::InvalidEndpoint(
                ENV_TIER1_SPICEDB_ENDPOINT.into(),
            ));
        }
    };
    let token = match env.var(ENV_TIER1_SPICEDB_TOKEN) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(SpiceDbImportGateBuildError::InvalidToken(ENV_TIER1_SPICEDB_TOKEN.into()));
        }
    };

    match (endpoint, token) {
        (None, None) => Ok(None),
        (Some(endpoint), Some(token)) => Ok(Some((SpiceDbEndpoint::parse(endpoint)?, SpiceDbToken::parse(token)?))),
        (Some(_), None) => Err(SpiceDbImportGateBuildError::Connect(format!(
            "{ENV_TIER1_SPICEDB_TOKEN} is required when {ENV_TIER1_SPICEDB_ENDPOINT} is set"
        ))),
        (None, Some(_)) => Err(SpiceDbImportGateBuildError::Connect(format!(
            "{ENV_TIER1_SPICEDB_ENDPOINT} is required when {ENV_TIER1_SPICEDB_TOKEN} is set"
        ))),
    }
}

pub struct AgentViewGateLayer {
    pub gate: Arc<dyn AgentViewGate>,
}

impl AgentViewGateLayer {
    pub async fn from_env<E: trogon_std::env::ReadEnv>(env: &E) -> Result<Self, SpiceDbImportGateBuildError> {
        let ttl = tier1_zed_token_ttl(env)?;
        if !tier1_enabled(env) {
            return Ok(Self {
                gate: Arc::new(NoopAgentViewGate),
            });
        }

        let Some((endpoint, token)) = optional_tier1_credentials(env)? else {
            return Err(SpiceDbImportGateBuildError::Connect(format!(
                "{ENV_TIER1_SPICEDB_ENABLED}=on requires {ENV_TIER1_SPICEDB_ENDPOINT} and {ENV_TIER1_SPICEDB_TOKEN}"
            )));
        };

        let gate = LiveAgentViewGate::connect(&endpoint, &token, ttl).await?;
        Ok(Self {
            gate: Arc::new(gate),
        })
    }
}

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

fn parse_subject_id(raw: &str) -> Option<String> {
    if let Some((_object_type, object_id)) = raw.split_once('/')
        && !object_id.is_empty()
    {
        return Some(object_id.to_owned());
    }
    if let Some((_object_type, object_id)) = raw.split_once(':')
        && !object_id.is_empty()
    {
        return Some(object_id.to_owned());
    }
    if !raw.is_empty() {
        Some(raw.to_owned())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_agents_maps_outcomes() {
        let agents = vec![A2aAgentId::new("a").unwrap(), A2aAgentId::new("b").unwrap()];
        let filtered = filter_agents_by_view(
            &agents,
            &[AgentViewCheckOutcome::Allowed, AgentViewCheckOutcome::Denied],
        );
        assert!(matches!(filtered[0], DiscoveryAgentFilterOutcome::Visible(_)));
        assert!(matches!(
            filtered[1],
            DiscoveryAgentFilterOutcome::Hidden {
                reason: DiscoveryHiddenReason::Denied,
                ..
            }
        ));
    }
}
