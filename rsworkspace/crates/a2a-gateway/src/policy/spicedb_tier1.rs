use std::fmt;
use std::sync::{Arc, LazyLock};

use a2a_nats::A2aMethod;
use a2a_nats::agent_id::A2aAgentId;
use a2a_pack::resource_tuples::{Tier1A2aMethodSlug, Tier1ResourceTupleTable};
use a2a_nats::catalog::import_gate::{
    BulkImportPermissionCheck, SpiceDbEndpoint, SpiceDbImportGateBuildError, SpiceDbPrincipal, SpiceDbToken,
    ZedTokenTtl,
};
use a2a_nats::catalog::import_gate::LiveBulkImportPermissionClient;
use a2a_nats::catalog::import_gate::parse_subject_reference;
use a2a_nats::catalog::spicedb_permission::{
    AgentViewCheckOutcome, AgentViewGate, LiveAgentViewGate, SpiceDbSessionCache, SpiceDbSessionKey,
};
use async_trait::async_trait;
use authzed::v1::check_bulk_permissions_pair;
use authzed::v1::check_permission_response::Permissionship;
use authzed::v1::relationship_update::Operation;
use authzed::v1::{
    CheckBulkPermissionsRequest, CheckBulkPermissionsRequestItem, Consistency, ObjectReference, Relationship,
    RelationshipUpdate, SubjectReference, WriteRelationshipsRequest, ZedToken,
};
use serde_json::Value;
use tonic::Status;
use trogon_std::env::ReadEnv;

pub const ENV_TIER1_SPICEDB_ENABLED: &str = "A2A_GATEWAY_TIER1_SPICEDB_ENABLED";
pub const ENV_TIER1_SPICEDB_ENDPOINT: &str = "A2A_GATEWAY_TIER1_SPICEDB_ENDPOINT";
pub const ENV_TIER1_SPICEDB_TOKEN: &str = "A2A_GATEWAY_TIER1_SPICEDB_TOKEN";
pub const ENV_TIER1_ZEDTOKEN_TTL_SECS: &str = "A2A_GATEWAY_TIER1_ZEDTOKEN_TTL_SECS";

const DEFAULT_TIER1_ZEDTOKEN_TTL_SECS: u64 = 60;

static TIER1_RESOURCE_TUPLE_TABLE: LazyLock<Tier1ResourceTupleTable> =
    LazyLock::new(Tier1ResourceTupleTable::bundled);

pub use a2a_pack::resource_tuples::{
    Tier1DeriveError, Tier1Permission, Tier1ResourceId, Tier1ResourceTuple, Tier1ResourceType,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier1OwnerTuple {
    pub resource_type: Tier1ResourceType,
    pub resource_id: Tier1ResourceId,
    pub relation: String,
    pub subject_type: String,
    pub subject_id: String,
}

impl Tier1OwnerTuple {
    pub fn for_task(agent_id: &A2aAgentId, task_id: &str, subject_type: &str, subject_id: &str) -> Self {
        let tuple = Tier1ResourceTuple::new(
            "task",
            format!("{}:{}", agent_id.as_str(), task_id),
            "owner",
        );
        Self {
            resource_type: tuple.resource_type,
            resource_id: tuple.resource_id,
            relation: "owner".into(),
            subject_type: subject_type.to_owned(),
            subject_id: subject_id.to_owned(),
        }
    }
}

pub type Tier1SessionKey = SpiceDbSessionKey;
pub type SpiceDbTier1SessionCache = SpiceDbSessionCache;

pub fn a2a_method_from_dots(method_dots: &str) -> Option<A2aMethod> {
    match method_dots {
        "message.send" => Some(A2aMethod::MessageSend),
        "message.stream" => Some(A2aMethod::MessageStream),
        "tasks.get" => Some(A2aMethod::TasksGet),
        "tasks.list" => Some(A2aMethod::TasksList),
        "tasks.cancel" => Some(A2aMethod::TasksCancel),
        "tasks.resubscribe" => Some(A2aMethod::TasksResubscribe),
        "tasks.push_notification_config.set" => Some(A2aMethod::PushNotificationSet),
        "tasks.push_notification_config.get" => Some(A2aMethod::PushNotificationGet),
        "tasks.push_notification_config.list" => Some(A2aMethod::PushNotificationList),
        "tasks.push_notification_config.delete" => Some(A2aMethod::PushNotificationDelete),
        "agent.card" => Some(A2aMethod::AgentCard),
        _ => None,
    }
}

pub fn derive_tuple(
    method: &A2aMethod,
    agent_id: &A2aAgentId,
    publisher_account: &str,
    params: &Value,
) -> Result<Tier1ResourceTuple, Tier1DeriveError> {
    TIER1_RESOURCE_TUPLE_TABLE.derive(
        &Tier1A2aMethodSlug::new(method.as_str()),
        agent_id.as_str(),
        publisher_account,
        params,
    )
}

fn task_id_from_params(params: &Value) -> Option<String> {
    params
        .get("id")
        .or_else(|| params.get("task_id"))
        .or_else(|| params.get("taskId"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tier1SpiceDbBuildError {
    InvalidEndpoint(String),
    InvalidToken(String),
    InvalidZedTokenTtl(String),
    Connect(String),
    PartialConfig(String),
}

impl fmt::Display for Tier1SpiceDbBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEndpoint(msg) => write!(f, "invalid tier-1 SpiceDB endpoint: {msg}"),
            Self::InvalidToken(msg) => write!(f, "invalid tier-1 SpiceDB token: {msg}"),
            Self::InvalidZedTokenTtl(msg) => write!(f, "invalid tier-1 ZedToken TTL: {msg}"),
            Self::Connect(msg) => write!(f, "tier-1 SpiceDB connect failed: {msg}"),
            Self::PartialConfig(msg) => write!(f, "partial tier-1 SpiceDB configuration: {msg}"),
        }
    }
}

impl std::error::Error for Tier1SpiceDbBuildError {}

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

fn tier1_enabled<E: ReadEnv>(env: &E) -> bool {
    match env.var(ENV_TIER1_SPICEDB_ENABLED) {
        Ok(raw) => matches!(raw.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(std::env::VarError::NotPresent) => false,
        Err(std::env::VarError::NotUnicode(_)) => false,
    }
}

fn tier1_zed_token_ttl<E: ReadEnv>(env: &E) -> Result<ZedTokenTtl, Tier1SpiceDbBuildError> {
    match env.var(ENV_TIER1_ZEDTOKEN_TTL_SECS) {
        Ok(raw) => raw
            .parse::<u64>()
            .map(ZedTokenTtl::from_secs)
            .map_err(|_| Tier1SpiceDbBuildError::InvalidZedTokenTtl(raw)),
        Err(std::env::VarError::NotPresent) => Ok(ZedTokenTtl::from_secs(DEFAULT_TIER1_ZEDTOKEN_TTL_SECS)),
        Err(std::env::VarError::NotUnicode(_)) => Err(Tier1SpiceDbBuildError::InvalidZedTokenTtl(
            ENV_TIER1_ZEDTOKEN_TTL_SECS.into(),
        )),
    }
}

fn optional_tier1_credentials<E: ReadEnv>(
    env: &E,
) -> Result<Option<(SpiceDbEndpoint, SpiceDbToken)>, Tier1SpiceDbBuildError> {
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
        (Some(endpoint), Some(token)) => Ok(Some((SpiceDbEndpoint::parse(endpoint)?, SpiceDbToken::parse(token)?))),
        (Some(_), None) => Err(Tier1SpiceDbBuildError::PartialConfig(format!(
            "{ENV_TIER1_SPICEDB_TOKEN} is required when {ENV_TIER1_SPICEDB_ENDPOINT} is set"
        ))),
        (None, Some(_)) => Err(Tier1SpiceDbBuildError::PartialConfig(format!(
            "{ENV_TIER1_SPICEDB_ENDPOINT} is required when {ENV_TIER1_SPICEDB_TOKEN} is set"
        ))),
    }
}

pub struct Tier1SpiceDbConfig;

pub struct GatewayTier1Layer {
    pub gate: Arc<dyn SpiceDbTier1Gate>,
    pub owner_emitter: Option<Arc<dyn OwnerTupleEmitter>>,
    pub discovery_view: Arc<dyn AgentViewGate>,
}

impl Tier1SpiceDbConfig {
    pub async fn from_env<E: ReadEnv>(env: &E) -> Result<GatewayTier1Layer, Tier1SpiceDbBuildError> {
        let ttl = tier1_zed_token_ttl(env)?;
        if !tier1_enabled(env) {
            return Ok(GatewayTier1Layer {
                gate: Arc::new(NoopSpiceDbTier1Gate),
                owner_emitter: None,
                discovery_view: Arc::new(a2a_nats::catalog::spicedb_permission::NoopAgentViewGate),
            });
        }

        let Some((endpoint, token)) = optional_tier1_credentials(env)? else {
            return Err(Tier1SpiceDbBuildError::PartialConfig(format!(
                "{ENV_TIER1_SPICEDB_ENABLED}=on requires {ENV_TIER1_SPICEDB_ENDPOINT} and {ENV_TIER1_SPICEDB_TOKEN}"
            )));
        };

        let client = LiveBulkImportPermissionClient::connect(&endpoint, &token).await?;
        let session_cache = SpiceDbTier1SessionCache::new(ttl);
        let client = Arc::new(client);
        let discovery_view = Arc::new(LiveAgentViewGate::new(client.clone(), session_cache.clone()));
        let live = Arc::new(LiveSpiceDbTier1Gate::new(client, session_cache));
        Ok(GatewayTier1Layer {
            gate: live.clone(),
            owner_emitter: Some(live),
            discovery_view,
        })
    }
}

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

pub struct LiveSpiceDbTier1Gate {
    client: Arc<dyn BulkImportPermissionCheck>,
    session_cache: SpiceDbTier1SessionCache,
    view_gate: LiveAgentViewGate,
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
        let Some((subject_type, subject_id)) = spicedb_tier1_subject_from_principal(principal) else {
            tracing::warn!(
                session_sub = %session.sub(),
                session_account = %session.account(),
                "SpiceDbTier1Gate: denying — principal lacks subject mapping"
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
            Err(error) => {
                tracing::warn!(
                    session_sub = %session.sub(),
                    resource = %tuple.resource_id.as_str(),
                    error = %error,
                    "SpiceDbTier1Gate: Authzed BulkCheckPermissions transport error"
                );
                return Tier1AuthorizeOutcome::TransportError;
            }
        };

        let allowed = response.pairs.first().is_some_and(pair_is_allowed);
        if !allowed {
            match response.pairs.first().and_then(|pair| pair.response.as_ref()) {
                Some(check_bulk_permissions_pair::Response::Item(item)) => {
                    tracing::warn!(
                        session_sub = %session.sub(),
                        resource_type = %tuple.resource_type.as_str(),
                        resource_id = %tuple.resource_id.as_str(),
                        permission = %tuple.permission.as_str(),
                        permissionship = item.permissionship,
                        "SpiceDbTier1Gate: BulkCheckPermissions denied"
                    );
                }
                Some(check_bulk_permissions_pair::Response::Error(error)) => {
                    tracing::warn!(
                        session_sub = %session.sub(),
                        resource_type = %tuple.resource_type.as_str(),
                        resource_id = %tuple.resource_id.as_str(),
                        permission = %tuple.permission.as_str(),
                        error = ?error,
                        "SpiceDbTier1Gate: BulkCheckPermissions item error"
                    );
                }
                None => {
                    tracing::warn!(
                        session_sub = %session.sub(),
                        "SpiceDbTier1Gate: BulkCheckPermissions empty pair response"
                    );
                }
            }
            return Tier1AuthorizeOutcome::Denied;
        }

        let zed_token = response.checked_at.map(|zed| zed.token);
        if let Some(token) = zed_token.clone() {
            self.session_cache.insert(session.clone(), token).await;
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

pub fn tier1_session_from_principal(principal: &SpiceDbPrincipal, fallback_account: &str) -> Option<Tier1SessionKey> {
    a2a_nats::catalog::spicedb_permission::session_from_principal(principal, fallback_account)
}

pub fn tier1_principal_from_caller(caller_slug: &str, account: &str) -> SpiceDbPrincipal {
    SpiceDbPrincipal(serde_json::json!({
        "spicedb_subject": format!("user/{caller_slug}"),
        "sub": caller_slug,
        "session_account": account,
    }))
}

fn spicedb_tier1_subject_from_principal(principal: &SpiceDbPrincipal) -> Option<(String, String)> {
    principal
        .0
        .get("spicedb_subject")
        .and_then(serde_json::Value::as_str)
        .and_then(parse_subject_reference)
}

pub fn owner_tuple_for_message_send(
    agent_id: &A2aAgentId,
    params: &Value,
    principal: &SpiceDbPrincipal,
) -> Option<Tier1OwnerTuple> {
    let task_id = task_id_from_params(params)?;
    let (subject_type, subject_id) = spicedb_tier1_subject_from_principal(principal)?;
    Some(Tier1OwnerTuple::for_task(agent_id, &task_id, &subject_type, &subject_id))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use std::time::Duration;

    use authzed::v1::CheckBulkPermissionsResponseItem;
    use trogon_std::env::InMemoryEnv;

    use super::*;

    struct MockTier1Client {
        responses: Mutex<Vec<Result<authzed::v1::CheckBulkPermissionsResponse, Status>>>,
        write_responses: Mutex<Vec<Result<authzed::v1::WriteRelationshipsResponse, Status>>>,
        check_requests: Mutex<Vec<CheckBulkPermissionsRequest>>,
        write_requests: Mutex<Vec<WriteRelationshipsRequest>>,
    }

    impl MockTier1Client {
        fn new(responses: Vec<Result<authzed::v1::CheckBulkPermissionsResponse, Status>>) -> Self {
            Self {
                responses: Mutex::new(responses),
                write_responses: Mutex::new(Vec::new()),
                check_requests: Mutex::new(Vec::new()),
                write_requests: Mutex::new(Vec::new()),
            }
        }

        fn with_write_responses(
            responses: Vec<Result<authzed::v1::CheckBulkPermissionsResponse, Status>>,
            write_responses: Vec<Result<authzed::v1::WriteRelationshipsResponse, Status>>,
        ) -> Self {
            Self {
                responses: Mutex::new(responses),
                write_responses: Mutex::new(write_responses),
                check_requests: Mutex::new(Vec::new()),
                write_requests: Mutex::new(Vec::new()),
            }
        }

        fn write_count(&self) -> usize {
            self.write_requests.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl BulkImportPermissionCheck for MockTier1Client {
        async fn check_bulk_permissions(
            &self,
            request: CheckBulkPermissionsRequest,
        ) -> Result<authzed::v1::CheckBulkPermissionsResponse, Status> {
            self.check_requests.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop()
                .unwrap_or_else(|| Err(Status::unavailable("no mock response")))
        }

        async fn write_relationships(
            &self,
            request: WriteRelationshipsRequest,
        ) -> Result<authzed::v1::WriteRelationshipsResponse, Status> {
            self.write_requests.lock().unwrap().push(request);
            self.write_responses
                .lock()
                .unwrap()
                .pop()
                .unwrap_or_else(|| Ok(authzed::v1::WriteRelationshipsResponse { written_at: None }))
        }
    }

    fn allowed_response(token: &str) -> authzed::v1::CheckBulkPermissionsResponse {
        authzed::v1::CheckBulkPermissionsResponse {
            pairs: vec![authzed::v1::CheckBulkPermissionsPair {
                request: None,
                response: Some(check_bulk_permissions_pair::Response::Item(
                    CheckBulkPermissionsResponseItem {
                        permissionship: Permissionship::HasPermission as i32,
                        partial_caveat_info: None,
                        debug_trace: None,
                    },
                )),
            }],
            checked_at: Some(ZedToken {
                token: token.to_owned(),
            }),
        }
    }

    fn denied_response() -> authzed::v1::CheckBulkPermissionsResponse {
        authzed::v1::CheckBulkPermissionsResponse {
            pairs: vec![authzed::v1::CheckBulkPermissionsPair {
                request: None,
                response: Some(check_bulk_permissions_pair::Response::Item(
                    CheckBulkPermissionsResponseItem {
                        permissionship: Permissionship::NoPermission as i32,
                        partial_caveat_info: None,
                        debug_trace: None,
                    },
                )),
            }],
            checked_at: None,
        }
    }

    fn agent() -> A2aAgentId {
        A2aAgentId::new("planner").unwrap()
    }

    fn principal() -> SpiceDbPrincipal {
        SpiceDbPrincipal(serde_json::json!({"spicedb_subject": "user/alice", "sub": "alice", "account": "acct"}))
    }

    fn session() -> Tier1SessionKey {
        Tier1SessionKey::new("alice", "acct")
    }

    #[tokio::test]
    async fn disabled_gate_is_noop() {
        let gate = NoopSpiceDbTier1Gate;
        assert!(!gate.is_enabled());
        let tuple = Tier1ResourceTuple::new("agent", "planner", "invoke");
        match gate.authorize(&session(), &principal(), &tuple).await {
            Tier1AuthorizeOutcome::Allowed { zed_token: None } => {}
            other => panic!("expected allow noop, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn from_env_disabled_builds_noop_gate() {
        let env = InMemoryEnv::new();
        let layer = Tier1SpiceDbConfig::from_env(&env).await.expect("disabled env");
        assert!(!layer.gate.is_enabled());
        assert!(layer.owner_emitter.is_none());
        assert!(!layer.discovery_view.is_enabled());
    }

    #[tokio::test]
    async fn from_env_partial_config_is_rejected() {
        let env = InMemoryEnv::new();
        env.set(ENV_TIER1_SPICEDB_ENABLED, "on");
        env.set(ENV_TIER1_SPICEDB_ENDPOINT, "https://spicedb.example.com:443");
        match Tier1SpiceDbConfig::from_env(&env).await {
            Err(Tier1SpiceDbBuildError::PartialConfig(_)) => {}
            Ok(_) => panic!("expected partial config error"),
            Err(other) => panic!("expected partial config error, got {other}"),
        }
    }

    #[tokio::test]
    async fn allowed_authorize_caches_zed_token_and_returns_snapshot() {
        let mock = Arc::new(MockTier1Client::new(vec![
            Ok(allowed_response("zed-2")),
            Ok(allowed_response("zed-1")),
        ]));
        let gate = LiveSpiceDbTier1Gate::new(mock.clone(), SpiceDbTier1SessionCache::new(ZedTokenTtl::from_secs(60)));
        let tuple = Tier1ResourceTuple::new("agent", "planner", "invoke");

        match gate.authorize(&session(), &principal(), &tuple).await {
            Tier1AuthorizeOutcome::Allowed {
                zed_token: Some(token),
            } if token == "zed-1" => {}
            other => panic!("unexpected first outcome: {other:?}"),
        }

        match gate.authorize(&session(), &principal(), &tuple).await {
            Tier1AuthorizeOutcome::Allowed {
                zed_token: Some(token),
            } if token == "zed-2" => {}
            other => panic!("unexpected second outcome: {other:?}"),
        }

        let requests = mock.check_requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].consistency.is_some());
    }

    #[tokio::test]
    async fn denied_authorize_returns_denied() {
        let mock = Arc::new(MockTier1Client::new(vec![Ok(denied_response())]));
        let gate = LiveSpiceDbTier1Gate::new(mock, SpiceDbTier1SessionCache::new(ZedTokenTtl::from_secs(60)));
        let tuple = Tier1ResourceTuple::new("agent", "planner", "invoke");
        assert_eq!(
            gate.authorize(&session(), &principal(), &tuple).await,
            Tier1AuthorizeOutcome::Denied
        );
    }

    #[tokio::test]
    async fn transport_error_returns_transport_error() {
        let mock = Arc::new(MockTier1Client::new(vec![Err(Status::unavailable("down"))]));
        let gate = LiveSpiceDbTier1Gate::new(mock, SpiceDbTier1SessionCache::new(ZedTokenTtl::from_secs(60)));
        let tuple = Tier1ResourceTuple::new("agent", "planner", "invoke");
        assert_eq!(
            gate.authorize(&session(), &principal(), &tuple).await,
            Tier1AuthorizeOutcome::TransportError
        );
    }

    #[tokio::test]
    async fn owner_tuple_emit_invoked_once_per_task() {
        let mock = Arc::new(MockTier1Client::new(vec![Ok(allowed_response("zed"))]));
        let gate = LiveSpiceDbTier1Gate::new(mock.clone(), SpiceDbTier1SessionCache::new(ZedTokenTtl::from_secs(60)));
        let owner = Tier1OwnerTuple::for_task(&agent(), "task-1", "user", "alice");
        gate.emit_owner(&owner).await.expect("write ok");
        assert_eq!(mock.write_count(), 1);
    }

    #[tokio::test]
    async fn owner_tuple_write_transport_failure_is_err() {
        let mock = Arc::new(MockTier1Client::with_write_responses(
            vec![],
            vec![Err(Status::unavailable("write down"))],
        ));
        let gate = LiveSpiceDbTier1Gate::new(mock, SpiceDbTier1SessionCache::new(ZedTokenTtl::from_secs(60)));
        let owner = Tier1OwnerTuple::for_task(&agent(), "task-1", "user", "alice");
        assert!(gate.emit_owner(&owner).await.is_err());
    }

    #[test]
    fn derive_tuple_message_send_uses_agent_invoke() {
        let tuple = derive_tuple(
            &A2aMethod::MessageSend,
            &agent(),
            "pub",
            &serde_json::json!({}),
        )
        .unwrap();
        assert_eq!(tuple.resource_type.as_str(), "agent");
        assert_eq!(tuple.resource_id.as_str(), "planner");
        assert_eq!(tuple.permission.as_str(), "invoke");
    }

    #[test]
    fn derive_tuple_tasks_list_uses_discover() {
        let tuple = derive_tuple(
            &A2aMethod::TasksList,
            &agent(),
            "pub",
            &serde_json::json!({}),
        )
        .unwrap();
        assert_eq!(tuple.permission.as_str(), "discover");
    }

    #[test]
    fn derive_tuple_agent_card_uses_agent_card_resource() {
        let tuple = derive_tuple(
            &A2aMethod::AgentCard,
            &agent(),
            "publisher",
            &serde_json::json!({}),
        )
        .unwrap();
        assert_eq!(tuple.resource_type.as_str(), "agent_card");
        assert_eq!(tuple.resource_id.as_str(), "publisher/planner");
    }

    #[test]
    fn derive_tuple_tasks_get_requires_task_id() {
        let err = derive_tuple(&A2aMethod::TasksGet, &agent(), "pub", &serde_json::json!({})).unwrap_err();
        assert_eq!(err, Tier1DeriveError::MissingTaskId);
    }

    #[test]
    fn session_cache_evicts_expired_entries() {
        let cache = SpiceDbTier1SessionCache::new(ZedTokenTtl::from_secs(1));
        let key = Tier1SessionKey::new("alice", "acct");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            cache.insert(key.clone(), "zed-old".into()).await;
            assert!(cache.get(&key).await.is_some());
        });
        std::thread::sleep(Duration::from_millis(1_100));
        rt.block_on(async {
            assert!(cache.get(&key).await.is_none());
        });
    }
}
