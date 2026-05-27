#![cfg(feature = "live")]

mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_nats::HeaderMap;
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::StreamExt;
use support::{
    AGENT1, AGENT2, TEST_PURPOSE, WKLA, WKLB, bootstrap_claims, build_exchange_service, decode_payload,
    kv_agent_record, mint_bootstrap_token, shared_test_keys, spawn_nats,
};
use trogon_a2a_sdk::constants::CALLER_JWT_HEADER;
use trogon_a2a_sdk::registry::NatsRegistry;
use trogon_a2a_sdk::sts::NatsSts;
use trogon_a2a_sdk::traits::{MessageTransport, SubjectTokenSource, SvidSource};
use trogon_a2a_sdk::{
    AgentId, Audience, Caller, Client, Handler, Purpose, Rs256Jwks, SdkError, serve,
};
use trogon_agent_registry::{AgentRecord, LOOKUP_SUBJECT, LookupRequest, LookupResponse, QUEUE_GROUP};
use trogon_sts::EXCHANGE_SUBJECT;
use trogon_sts::types::StsExchangeRequest;

struct FixedToken(String);

#[async_trait]
impl SubjectTokenSource for FixedToken {
    async fn current(&self) -> Result<String, SdkError> {
        Ok(self.0.clone())
    }
}

struct FixedSvid(String);

#[async_trait]
impl SvidSource for FixedSvid {
    async fn current(&self) -> Result<String, SdkError> {
        Ok(self.0.clone())
    }
}

struct RecordingHandler {
    caller: Arc<Mutex<Option<Caller>>>,
}

#[async_trait]
impl Handler for RecordingHandler {
    async fn handle(&self, caller: Caller, raw_payload: Bytes) -> Result<Bytes, SdkError> {
        *self.caller.lock().unwrap() = Some(caller);
        Ok(raw_payload)
    }
}

struct CapturingTransport {
    inner: async_nats::Client,
    jwt: Arc<Mutex<Option<String>>>,
}

#[async_trait]
impl MessageTransport for CapturingTransport {
    async fn request(&self, subject: &str, payload: Bytes, headers: HeaderMap) -> Result<Bytes, SdkError> {
        if let Some(jwt) = headers.get(CALLER_JWT_HEADER) {
            *self.jwt.lock().unwrap() = Some(jwt.as_str().to_owned());
        }
        self.inner
            .request_with_headers(subject.to_owned(), headers, payload)
            .await
            .map(|msg| msg.payload)
            .map_err(SdkError::nats)
    }
}

async fn spawn_sts_consumer(
    nats: async_nats::Client,
    service: Arc<trogon_sts::exchange::ExchangeService<
        trogon_sts::registry::InMemoryRegistry,
        trogon_sts::audit::RecordingAuditPublisher,
    >>,
) {
    tokio::spawn(async move {
        let mut sub = nats
            .queue_subscribe(EXCHANGE_SUBJECT.to_string(), "trogon-a2a-sdk-live-sts".to_string())
            .await
            .expect("sts subscribe");
        while let Some(msg) = sub.next().await {
            let Some(reply) = msg.reply else { continue };
            let request: StsExchangeRequest = match serde_json::from_slice(&msg.payload) {
                Ok(req) => req,
                Err(_) => continue,
            };
            let body = match service.handle(request, None).await {
                Ok(ok) => serde_json::to_vec(&ok).unwrap_or_default(),
                Err(err) => serde_json::to_vec(&err).unwrap_or_default(),
            };
            let _ = nats.publish(reply, body.into()).await;
        }
    });
}

async fn spawn_registry_lookup_responder(nats: async_nats::Client, records: HashMap<String, AgentRecord>) {
    let records = Arc::new(records);
    tokio::spawn(async move {
        let mut sub = nats
            .queue_subscribe(LOOKUP_SUBJECT.to_string(), QUEUE_GROUP.to_string())
            .await
            .expect("registry subscribe");
        while let Some(msg) = sub.next().await {
            let Some(reply) = msg.reply else { continue };
            let Ok(request) = serde_json::from_slice::<LookupRequest>(&msg.payload) else {
                continue;
            };
            let response = match records.get(&request.agent_id) {
                Some(record) => LookupResponse::Found { record: record.clone() },
                None => LookupResponse::NotFound,
            };
            let body = serde_json::to_vec(&response).expect("encode lookup response");
            let _ = nats.publish(reply, body.into()).await;
        }
    });
}

#[tokio::test]
#[cfg_attr(not(feature = "live"), ignore)]
async fn client_call_and_serve_round_trip_over_live_nats() {
    let Some((_nats_proc, url)) = spawn_nats().await else {
        eprintln!("skipping live test: nats-server not available on PATH");
        return;
    };

    let nats = async_nats::connect(&url).await.expect("connect");
    let mut registry_records = HashMap::new();
    registry_records.insert(
        AGENT1.to_string(),
        kv_agent_record(
            AGENT1,
            WKLA,
            vec!["urn:trogon:a2a:agent:acme:agent2".into()],
        ),
    );
    registry_records.insert(
        AGENT2.to_string(),
        kv_agent_record(
            AGENT2,
            WKLB,
            vec!["urn:trogon:a2a:agent:acme:agent2".into()],
        ),
    );
    spawn_registry_lookup_responder(nats.clone(), registry_records).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let keys = shared_test_keys();
    let sts_service = Arc::new(build_exchange_service(keys));
    spawn_sts_consumer(nats.clone(), Arc::clone(&sts_service)).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let agent2 = AgentId::parse(AGENT2).expect("agent2 id");
    let caller_box = Arc::new(Mutex::new(None));
    let handler = RecordingHandler {
        caller: Arc::clone(&caller_box),
    };
    let jwks = Rs256Jwks::new(keys.mesh_jwks.clone());
    let serve_nats = nats.clone();
    let serve_agent2 = agent2.clone();
    let serve_task = tokio::spawn(async move {
        serve(serve_nats, serve_agent2, jwks, handler).await
    });

    let captured_jwt = Arc::new(Mutex::new(None));
    let bootstrap = mint_bootstrap_token(keys, bootstrap_claims(keys, AGENT1));
    let client = Client::builder()
        .agent_id(AgentId::parse(AGENT1).expect("agent1 id"))
        .svid_source(FixedSvid(WKLA.into()))
        .subject_token_source(FixedToken(bootstrap))
        .sts(NatsSts::new(nats.clone()))
        .registry(NatsRegistry::new(nats.clone()))
        .transport(CapturingTransport {
            inner: nats.clone(),
            jwt: Arc::clone(&captured_jwt),
        })
        .build()
        .expect("client");

    let target = AgentId::parse(AGENT2).expect("target");
    let payload = serde_json::json!({"msg": "live"});
    let reply: serde_json::Value = client
        .call(&target, &payload, Some(&Purpose::new(TEST_PURPOSE)))
        .await
        .expect("call");
    assert_eq!(reply.get("msg").and_then(|v| v.as_str()), Some("live"));

    let jwt = captured_jwt.lock().unwrap().clone().expect("outbound jwt");
    let claims = decode_payload(&jwt);
    assert_eq!(
        claims.get("aud").and_then(|v| v.as_str()),
        Some(Audience::for_agent(&target).as_str())
    );
    let chain = claims
        .get("act_chain")
        .and_then(|v| v.as_array())
        .expect("act_chain");
    assert_eq!(
        chain.last().and_then(|e| e.get("agent_id")).and_then(|v| v.as_str()),
        Some(AGENT1)
    );

    let caller = caller_box.lock().unwrap().clone().expect("serve caller");
    assert_eq!(caller.originator.sub, "user:alice");
    assert_eq!(caller.direct.agent_id.as_deref(), Some(AGENT1));
    assert_eq!(caller.purpose.as_ref().map(Purpose::as_str), Some(TEST_PURPOSE));
    assert_eq!(caller.chain.len(), 1);

    serve_task.abort();
}
