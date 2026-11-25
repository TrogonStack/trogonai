#![allow(dead_code)]
use axum::{Json, Router, routing::get};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use mcp_postgres::{
    AuthenticationConfigInput, Authenticator, HttpTransportConfig, HttpTransportConfigInput, QueryService, http_router,
};
use rmcp::{
    ServiceExt,
    service::{RoleClient, RunningService},
    transport::{StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig},
};
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{net::TcpListener, task::JoinHandle};

pub struct TestIssuer {
    pub issuer: String,
    pub jwks_uri: String,
    key: EncodingKey,
    task: JoinHandle<()>,
}
impl TestIssuer {
    pub async fn new() -> Self {
        let pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let public = pair.public_key_raw();
        let jwks = json!({"keys":[{"kty":"EC","crv":"P-256","alg":"ES256","use":"sig","kid":"test-key","x":URL_SAFE_NO_PAD.encode(&public[1..33]),"y":URL_SAFE_NO_PAD.encode(&public[33..65])}]});
        let key = EncodingKey::from_ec_pem(pair.serialize_pem().as_bytes()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let jwks_uri = format!("{issuer}/jwks");
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/jwks", get(move || async move { Json(jwks) })),
            )
            .await
            .unwrap();
        });
        Self {
            issuer,
            jwks_uri,
            key,
            task,
        }
    }
    pub fn authentication(&self) -> AuthenticationConfigInput {
        serde_json::from_value(json!({"issuers":[{
            "issuer":self.issuer,"jwks_uri":self.jwks_uri,"audience":"test-mcp",
            "required_scopes":["sql:query"],
            "users":{"test-user":"example-user"},"clients":{"test-client":"example-client"}
        }]}))
        .unwrap()
    }
    pub fn claims(&self) -> Value {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        json!({"iss":self.issuer,"aud":"test-mcp","sub":"test-user","client_id":"test-client","scope":"sql:query","iat":now,"exp":now+300})
    }
    pub fn token(&self) -> String {
        self.token_with_claims(self.claims())
    }
    pub fn token_with_claims(&self, claims: Value) -> String {
        self.token_with_key_id(claims, "test-key")
    }
    pub fn token_with_key_id(&self, claims: Value, key_id: &str) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(key_id.into());
        jsonwebtoken::encode(&header, &claims, &self.key).unwrap()
    }
    pub async fn stop(&self) {
        self.task.abort();
        while !self.task.is_finished() {
            tokio::task::yield_now().await;
        }
    }
}
impl Drop for TestIssuer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub struct HttpFixture {
    pub issuer: TestIssuer,
    pub url: String,
    pub token: String,
    task: JoinHandle<()>,
}
impl HttpFixture {
    pub async fn new(service: QueryService) -> Self {
        Self::with_origins(service, &[]).await
    }
    pub async fn with_origins(service: QueryService, origins: &[&str]) -> Self {
        let issuer = TestIssuer::new().await;
        let authenticator = Arc::new(Authenticator::new(issuer.authentication()).await.unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bind = listener.local_addr().unwrap();
        let url = format!("http://{bind}/mcp");
        let config = HttpTransportConfig::new(HttpTransportConfigInput {
            bind,
            resource_url: url.clone(),
            allowed_origins: origins.iter().map(|origin| (*origin).into()).collect(),
        })
        .unwrap();
        let router = http_router(service, authenticator, config);
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let token = issuer.token();
        Self {
            issuer,
            url,
            token,
            task,
        }
    }
    pub async fn client(&self) -> RunningService<RoleClient, ()> {
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(self.url.clone()).auth_header(self.token.clone()),
        );
        ().serve(transport).await.unwrap()
    }
}
impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
