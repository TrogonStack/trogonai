use std::sync::{Arc, Mutex};
use std::time::Duration;

use a2a_auth_callout::{AccountName, KeyVersion, StaticSigningKeySource};
use a2a_nats::A2aAgentId;
use a2a_nats::catalog::agent_view::AgentViewCheckOutcome;
use a2a_nats::catalog::import_gate::SpiceDbPrincipal;
use a2a_pack::resource_tuples::Tier1ResourceTuple;
use async_nats::{HeaderMap, Message, Subscriber};
use bytes::Bytes;
use futures::StreamExt;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::{CoreTestServer, JetStreamTestServer};
use trogon_std::env::InMemoryEnv;

use crate::aauth::GatewayAAuthIngress;
use crate::config::{Args, Config, config_from_args};
use crate::constants::{ENV_GATEWAY_AUDIT_PUBLISH, ENV_GATEWAY_TRUST_CALLER_HEADERS};
use crate::gw_ingress_stream::{
    GatewayStreamingIngressConfig, StreamingIngressGate, StreamingMaxAckPending, StreamingMaxInflightPerCaller,
};
use crate::jwt_caller_identity::{JwtHeaderCallerIdentitySource, gateway_caller_identity_policy};
use crate::policy::spicedb_tier1::{
    NoopSpiceDbTier1Gate, OwnerTupleEmitter, SpiceDbTier1Gate, Tier1AuthorizeOutcome, Tier1OwnerTuple, Tier1SessionKey,
};
use crate::policy::tier1_declarative::{NoopTier1DeclarativeGate, Tier1DeclarativeGate};
use crate::runtime::dispatch::dispatch_gateway_ingress;
use crate::runtime::policy_stack::GatewayPolicyStack;

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, thiserror::Error)]
enum FixtureError {
    #[error("NATS subscription ended before the expected message")]
    SubscriptionEnded,
}

pub struct DispatchFixture<S = CoreTestServer> {
    _server: S,
    pub client: async_nats::Client,
    pub agents: Subscriber,
    pub replies: Subscriber,
    pub audits: Subscriber,
    pub env: InMemoryEnv,
    pub config: Config,
    pub tier1: Arc<dyn SpiceDbTier1Gate>,
    pub owner: Option<Arc<dyn OwnerTupleEmitter>>,
    pub declarative: Arc<dyn Tier1DeclarativeGate>,
    pub policy: GatewayPolicyStack,
    pub aauth: Option<GatewayAAuthIngress>,
    identity: JwtHeaderCallerIdentitySource,
    pub shutdown: CancellationToken,
    pub streaming_enabled: bool,
    pub streaming_config: GatewayStreamingIngressConfig,
    pub streaming_gate: StreamingIngressGate,
}

impl DispatchFixture {
    pub async fn new() -> TestResult<Self> {
        let server = CoreTestServer::start().await;
        let client = async_nats::connect(server.address()).await?;
        Self::with_client(server, client).await
    }
}

impl DispatchFixture<JetStreamTestServer> {
    pub async fn streaming() -> TestResult<Self> {
        let server = JetStreamTestServer::start().await;
        let client = server.client().await;
        let mut fixture = Self::with_client(server, client).await?;
        fixture.streaming_enabled = true;
        fixture.streaming_config =
            GatewayStreamingIngressConfig::new(StreamingMaxAckPending::new(32), StreamingMaxInflightPerCaller::new(1));
        fixture.streaming_gate = StreamingIngressGate::new(fixture.streaming_config);
        Ok(fixture)
    }
}

impl<S> DispatchFixture<S> {
    async fn with_client(server: S, client: async_nats::Client) -> TestResult<Self> {
        let env = InMemoryEnv::new();
        env.set(ENV_GATEWAY_AUDIT_PUBLISH, "true");
        env.set(ENV_GATEWAY_TRUST_CALLER_HEADERS, "true");
        let (config, _) = config_from_args(
            Args {
                nats_url: format!("{}:{}", client.server_info().host, client.server_info().port),
                prefix: "a2a".into(),
                queue_group: None,
            },
            &env,
        )?;
        let source = StaticSigningKeySource::new(&nkeys::KeyPair::new_account().seed()?, KeyVersion::new("test")?)?;
        let identity = JwtHeaderCallerIdentitySource::new(Arc::new(source), AccountName::new("tenant"));
        let agents = client.subscribe("a2a.v1.agents.>").await?;
        let replies = client.subscribe("_INBOX.dispatch").await?;
        let audits = client.subscribe("a2a.v1.audit.>").await?;
        client.flush().await?;
        let streaming_config = GatewayStreamingIngressConfig::from_env(&env);
        Ok(Self {
            _server: server,
            client,
            agents,
            replies,
            audits,
            env,
            config,
            tier1: Arc::new(NoopSpiceDbTier1Gate),
            owner: None,
            declarative: Arc::new(NoopTier1DeclarativeGate),
            policy: GatewayPolicyStack::noop(),
            aauth: None,
            identity,
            shutdown: CancellationToken::new(),
            streaming_enabled: false,
            streaming_config,
            streaming_gate: StreamingIngressGate::new(streaming_config),
        })
    }

    pub async fn dispatch(&self, message: Message) {
        dispatch_gateway_ingress(
            &self.client,
            &self.config,
            self.tier1.as_ref(),
            self.owner.as_ref(),
            self.declarative.as_ref(),
            self.aauth.as_ref(),
            &self.policy,
            &self.identity,
            gateway_caller_identity_policy(&self.env),
            self.streaming_enabled,
            self.streaming_config,
            &self.streaming_gate,
            self.shutdown.clone(),
            &self.env,
            message,
        )
        .await;
    }

    pub async fn audit(&mut self, outcome: &str) -> TestResult<Value> {
        let message = receive(&mut self.audits).await?;
        assert_eq!(message.subject.as_str(), format!("a2a.v1.audit.bot.{outcome}"));
        let body: Value = serde_json::from_slice(&message.payload)?;
        assert_eq!(body["agent_id"], "bot");
        assert_eq!(body["outcome"], outcome);
        Ok(body)
    }

    pub async fn denied(&mut self, code: i32, id: Value) -> TestResult<Value> {
        let response = receive(&mut self.replies).await?;
        let body: Value = serde_json::from_slice(&response.payload)?;
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], id);
        assert_eq!(body["error"]["code"], code);
        assert_empty(&self.client, &mut self.agents, "a2a.v1.agents.barrier").await?;
        Ok(body)
    }
}

pub fn request(method: &str, params: Value) -> Message {
    let mut headers = HeaderMap::new();
    headers.insert(a2a_nats::constants::GATEWAY_CALLER_ID_HEADER, "alice");
    Message {
        subject: format!("a2a.v1.gateway.bot.{method}").into(),
        reply: Some("_INBOX.dispatch".into()),
        payload: serde_json::to_vec(&json!({
            "jsonrpc": "2.0", "id": "request-7", "method": method.replace('.', "/"), "params": params,
        }))
        .expect("JSON value serializes")
        .into(),
        headers: Some(headers),
        status: None,
        description: None,
        length: 0,
    }
}

pub async fn receive(subscription: &mut Subscriber) -> TestResult<Message> {
    Ok(tokio::time::timeout(Duration::from_secs(5), subscription.next())
        .await?
        .ok_or(FixtureError::SubscriptionEnded)?)
}

pub async fn assert_empty(client: &async_nats::Client, subscription: &mut Subscriber, subject: &str) -> TestResult {
    // The marker shares the dispatch connection, so NATS delivers every earlier publish first.
    client
        .publish(subject.to_owned(), Bytes::from_static(b"barrier"))
        .await?;
    let next = receive(subscription).await?;
    assert_eq!(next.subject.as_str(), subject);
    assert_eq!(next.payload.as_ref(), b"barrier");
    Ok(())
}

#[derive(Clone)]
pub struct AuthorizationCall {
    pub session: Tier1SessionKey,
    pub principal: SpiceDbPrincipal,
    pub tuple: Tier1ResourceTuple,
}

pub struct RecordingTier1 {
    pub outcome: Tier1AuthorizeOutcome,
    pub calls: Mutex<Vec<AuthorizationCall>>,
}

impl RecordingTier1 {
    pub fn new(outcome: Tier1AuthorizeOutcome) -> Self {
        Self {
            outcome,
            calls: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl SpiceDbTier1Gate for RecordingTier1 {
    fn is_enabled(&self) -> bool {
        true
    }

    async fn authorize(
        &self,
        session: &Tier1SessionKey,
        principal: &SpiceDbPrincipal,
        tuple: &Tier1ResourceTuple,
    ) -> Tier1AuthorizeOutcome {
        self.calls.lock().expect("authorization calls").push(AuthorizationCall {
            session: session.clone(),
            principal: principal.clone(),
            tuple: tuple.clone(),
        });
        self.outcome.clone()
    }

    async fn bulk_check_agent_view(
        &self,
        _session: &Tier1SessionKey,
        _principal: &SpiceDbPrincipal,
        _agent_ids: &[A2aAgentId],
    ) -> Vec<AgentViewCheckOutcome> {
        panic!("dispatch must authorize the operation rather than discovery visibility")
    }
}

#[derive(Default)]
pub struct RecordingOwner {
    pub failure: Option<tonic::Status>,
    pub calls: Mutex<Vec<Tier1OwnerTuple>>,
}

#[async_trait::async_trait]
impl OwnerTupleEmitter for RecordingOwner {
    async fn emit_owner(&self, owner: &Tier1OwnerTuple) -> Result<(), tonic::Status> {
        self.calls.lock().expect("owner calls").push(owner.clone());
        self.failure.clone().map_or(Ok(()), Err)
    }
}
