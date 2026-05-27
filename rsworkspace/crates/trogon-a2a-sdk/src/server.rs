use std::collections::HashSet;

use async_nats::HeaderMap;
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::StreamExt;
use jsonwebtoken::{Algorithm, Validation, decode, decode_header};
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use opentelemetry::{Context, KeyValue, global};
use serde::Deserialize;

use crate::constants::{CALLER_JWT_HEADER, MAX_ACT_CHAIN_DEPTH};
use crate::subject::{agent_queue_group, agent_request_subject};
use crate::traits::Jwks;
use crate::types::{ActChainEntry, AgentId, Audience, Caller, Purpose, SdkError};

#[async_trait]
pub trait Handler: Send + Sync {
    async fn handle(&self, caller: Caller, raw_payload: Bytes) -> Result<Bytes, SdkError>;
}

#[derive(Debug, Deserialize)]
struct MeshClaims {
    aud: String,
    #[serde(default)]
    act_chain: Vec<ActChainEntry>,
    purpose: Option<String>,
    session_id: Option<String>,
}

pub(crate) async fn handle_inbound<J: Jwks + ?Sized>(
    own: &AgentId,
    jwks: &J,
    headers: &HeaderMap,
    payload: Bytes,
    handler: &impl Handler,
) -> Result<Bytes, SdkError> {
    let tracer = global::tracer("trogon-a2a-sdk");
    let own_audience = Audience::for_agent(own);
    let span = tracer
        .span_builder("a2a.serve.dispatch")
        .with_attributes([
            KeyValue::new("agent.id", own.to_string()),
            KeyValue::new("agent.call.direction", "inbound"),
        ])
        .start(&tracer);
    let cx = Context::current().with_span(span);
    let parent_cx = cx.clone();

    async {
        let token = extract_caller_jwt(headers)?;
        let caller = verify_token(jwks, &token, &own_audience).await?;
        let _attrs = tracer
            .span_builder("a2a.serve.caller")
            .with_attributes([
                KeyValue::new("agent.chain.depth", caller.chain_depth() as i64),
                KeyValue::new(
                    "agent.purpose",
                    caller.purpose.as_ref().map(Purpose::to_string).unwrap_or_default(),
                ),
            ])
            .start_with_context(&tracer, &parent_cx);
        handler.handle(caller, payload).await
    }
    .with_context(cx)
    .await
}

fn extract_caller_jwt(headers: &HeaderMap) -> Result<String, SdkError> {
    headers
        .get(CALLER_JWT_HEADER)
        .map(|v| v.as_str().to_owned())
        .ok_or_else(|| SdkError::InvalidToken("missing A2a-Caller-Jwt header".into()))
}

pub(crate) async fn verify_token<J: Jwks + ?Sized>(
    jwks: &J,
    token: &str,
    expected_aud: &Audience,
) -> Result<Caller, SdkError> {
    let header = decode_header(token).map_err(|e| SdkError::InvalidToken(format!("decode header: {e}")))?;
    let kid = header.kid.as_deref();
    let key = jwks.decoding_key(kid).await?;
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_aud = false;
    validation.validate_exp = false;
    validation.required_spec_claims.clear();
    let token_data =
        decode::<MeshClaims>(token, &key, &validation).map_err(|e| SdkError::InvalidToken(format!("verify: {e}")))?;

    if token_data.claims.aud != expected_aud.as_str() {
        return Err(SdkError::Forbidden(format!(
            "aud mismatch: expected {}, got {}",
            expected_aud.as_str(),
            token_data.claims.aud
        )));
    }

    verify_act_chain(&token_data.claims.act_chain)?;
    build_caller(token_data.claims)
}

fn verify_act_chain(chain: &[ActChainEntry]) -> Result<(), SdkError> {
    if chain.len() > MAX_ACT_CHAIN_DEPTH {
        return Err(SdkError::ChainTooDeep(chain.len()));
    }
    let mut seen = HashSet::new();
    for entry in chain {
        let agent_id = entry.agent_id.as_deref().unwrap_or("");
        let wkl = entry.wkl.as_deref().unwrap_or("");
        if !agent_id.is_empty() || !wkl.is_empty() {
            let key = (agent_id.to_owned(), wkl.to_owned());
            if !seen.insert(key.clone()) {
                return Err(SdkError::ChainLoop {
                    agent_id: key.0,
                    wkl: key.1,
                });
            }
        }
    }
    Ok(())
}

fn build_caller(claims: MeshClaims) -> Result<Caller, SdkError> {
    if claims.act_chain.is_empty() {
        return Err(SdkError::InvalidToken("empty act_chain".into()));
    }
    let originator = claims.act_chain[0].clone();
    let direct = claims
        .act_chain
        .last()
        .cloned()
        .ok_or_else(|| SdkError::InvalidToken("empty act_chain".into()))?;
    Ok(Caller {
        originator,
        chain: claims.act_chain,
        direct,
        purpose: claims.purpose.map(Purpose::new),
        session_id: claims.session_id,
    })
}

pub async fn serve<H, J>(nats: async_nats::Client, own: AgentId, jwks_source: J, handler: H) -> Result<(), SdkError>
where
    H: Handler + 'static,
    J: Jwks + 'static,
{
    let subject = agent_request_subject(&own);
    let queue = agent_queue_group(&own);
    let mut subscriber = nats.queue_subscribe(subject, queue).await.map_err(SdkError::nats)?;

    let handler = std::sync::Arc::new(handler);
    let jwks = std::sync::Arc::new(jwks_source);
    let own = std::sync::Arc::new(own);

    loop {
        let Some(message) = subscriber.next().await else {
            return Err(SdkError::transport_msg("subscription closed"));
        };
        let reply = message.reply.clone();
        let headers = message.headers.clone().unwrap_or_default();
        let payload = message.payload;
        let handler = handler.clone();
        let jwks = jwks.clone();
        let own = own.clone();

        let result = handle_inbound(own.as_ref(), jwks.as_ref(), &headers, payload, handler.as_ref()).await;

        if let Some(reply_subject) = reply {
            let response = match result {
                Ok(bytes) => bytes,
                Err(e) => {
                    tracing::warn!(error = %e, "inbound request failed");
                    Bytes::from(format!(r#"{{"error":"{e}"}}"#))
                }
            };
            if let Err(e) = nats.publish(reply_subject, response).await {
                return Err(SdkError::nats(e));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_nats::HeaderMap;
    use async_trait::async_trait;
    use bytes::Bytes;
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};

    use super::*;
    use crate::jwks::Hs256Jwks;
    use crate::types::ActChainEntry;

    const SECRET: &[u8] = b"test-secret-key-for-a2a-sdk";

    fn mint_token(aud: &str, chain: Vec<ActChainEntry>) -> String {
        let claims = serde_json::json!({
            "sub": "test-subject",
            "aud": aud,
            "exp": 4_000_000_000_i64,
            "act_chain": chain,
            "purpose": "test-purpose",
            "session_id": "sess-1",
        });
        let enc = EncodingKey::from_secret(SECRET);
        let mut hdr = Header::new(Algorithm::HS256);
        hdr.typ = Some("JWT".into());
        encode(&hdr, &claims, &enc).expect("encode")
    }

    fn entry(sub: &str, agent_id: &str, wkl: &str, iat: i64) -> ActChainEntry {
        ActChainEntry {
            sub: sub.to_owned(),
            agent_id: Some(agent_id.to_owned()),
            wkl: Some(wkl.to_owned()),
            iat,
        }
    }

    struct RecordingHandler {
        last_caller: Mutex<Option<Caller>>,
    }

    #[async_trait]
    impl Handler for RecordingHandler {
        async fn handle(&self, caller: Caller, _raw_payload: Bytes) -> Result<Bytes, SdkError> {
            *self.last_caller.lock().unwrap() = Some(caller);
            Ok(Bytes::from_static(b"ok"))
        }
    }

    #[tokio::test]
    async fn serve_rejects_aud_mismatch() {
        let own = AgentId::parse("acme/echo").unwrap();
        let expected_aud = Audience::for_agent(&own);
        let token = mint_token("urn:trogon:a2a:agent:acme:other", vec![entry("u", "a", "w", 1)]);
        let mut headers = HeaderMap::new();
        headers.insert(CALLER_JWT_HEADER, token.as_str());
        let jwks = Hs256Jwks::new(SECRET);
        let handler = RecordingHandler {
            last_caller: Mutex::new(None),
        };
        let err = handle_inbound(&own, &jwks, &headers, Bytes::from_static(b"{}"), &handler)
            .await
            .unwrap_err();
        assert!(matches!(err, SdkError::Forbidden(_)));
        let _ = expected_aud;
    }

    #[tokio::test]
    async fn serve_rejects_chain_depth_nine() {
        let own = AgentId::parse("acme/echo").unwrap();
        let aud = Audience::for_agent(&own);
        let chain: Vec<_> = (0..9)
            .map(|i| entry(&format!("hop-{i}"), &format!("agent-{i}"), &format!("wkl-{i}"), i))
            .collect();
        let token = mint_token(aud.as_str(), chain);
        let mut headers = HeaderMap::new();
        headers.insert(CALLER_JWT_HEADER, token.as_str());
        let jwks = Hs256Jwks::new(SECRET);
        let handler = RecordingHandler {
            last_caller: Mutex::new(None),
        };
        let err = handle_inbound(&own, &jwks, &headers, Bytes::from_static(b"{}"), &handler)
            .await
            .unwrap_err();
        assert!(matches!(err, SdkError::ChainTooDeep(9)));
    }

    #[tokio::test]
    async fn serve_rejects_agent_id_wkl_loop() {
        let own = AgentId::parse("acme/echo").unwrap();
        let aud = Audience::for_agent(&own);
        let chain = vec![
            entry("user", "acme/a", "wkl-a", 1),
            entry("agent-b", "acme/b", "wkl-b", 2),
            entry("agent-b", "acme/b", "wkl-b", 3),
        ];
        let token = mint_token(aud.as_str(), chain);
        let mut headers = HeaderMap::new();
        headers.insert(CALLER_JWT_HEADER, token.as_str());
        let jwks = Hs256Jwks::new(SECRET);
        let handler = RecordingHandler {
            last_caller: Mutex::new(None),
        };
        let err = handle_inbound(&own, &jwks, &headers, Bytes::from_static(b"{}"), &handler)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            SdkError::ChainLoop {
                agent_id,
                wkl
            } if agent_id == "acme/b" && wkl == "wkl-b"
        ));
    }

    #[tokio::test]
    async fn happy_path_round_trip_via_mocks() {
        let own = AgentId::parse("acme/echo").unwrap();
        let aud = Audience::for_agent(&own);
        let chain = vec![
            entry("user-alice", "acme/origin", "wkl-origin", 100),
            entry("agent-a", "acme/a", "wkl-a", 200),
            entry("agent-b", "acme/b", "wkl-b", 300),
        ];
        let token = mint_token(aud.as_str(), chain);
        let mut headers = HeaderMap::new();
        headers.insert(CALLER_JWT_HEADER, token.as_str());
        let jwks = Hs256Jwks::new(SECRET);
        let handler = RecordingHandler {
            last_caller: Mutex::new(None),
        };
        let response = handle_inbound(&own, &jwks, &headers, Bytes::from_static(b"hello"), &handler)
            .await
            .unwrap();
        assert_eq!(response, Bytes::from_static(b"ok"));
        let caller = handler.last_caller.lock().unwrap().take().unwrap();
        assert_eq!(caller.originator.sub, "user-alice");
        assert_eq!(caller.chain.len(), 3);
        assert_eq!(caller.direct.sub, "agent-b");
    }
}
