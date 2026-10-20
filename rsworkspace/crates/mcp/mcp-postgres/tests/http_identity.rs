#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod common;

use async_trait::async_trait;
use common::TestIssuer;
use mcp_postgres::{
    Authenticator, ConfigInput, Error, HttpTransportConfig, HttpTransportConfigInput, PermissionCheck,
    PermissionRequest, QueryService, RuntimeConfig, http_router,
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{ImageExt, runners::AsyncRunner},
};

#[derive(Default)]
struct Permissions {
    checked: Mutex<BTreeSet<(String, String)>>,
}

#[async_trait]
impl PermissionCheck for Permissions {
    async fn check_all(&self, requests: &[PermissionRequest]) -> Result<(), Error> {
        for request in requests {
            self.checked
                .lock()
                .unwrap()
                .insert((request.subject_kind.into(), request.subject.as_str().into()));
            if !matches!(
                (request.subject_kind, request.subject.as_str()),
                ("sql_user", "allowed-user") | ("sql_client", "allowed-client")
            ) {
                return Err(Error::Denied);
            }
        }
        Ok(())
    }
}

struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn call(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    arguments: Value,
    metadata: Value,
) -> reqwest::Response {
    client
        .post(url)
        .bearer_auth(token)
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
            "name":"query","arguments":arguments,"_meta":metadata
        }}))
        .send()
        .await
        .unwrap()
}

async fn success(response: reqwest::Response) {
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_ne!(body["result"]["isError"], true);
    let result: Value = serde_json::from_str(body["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(result["rows"], json!([[1]]));
}

async fn denied(response: reqwest::Response) {
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["result"]["isError"], true);
    assert_eq!(body["result"]["content"][0]["text"], "access denied");
}

#[tokio::test]
async fn http_requests_keep_issuer_namespaces_and_current_actor_permissions_separate() {
    let postgres = Postgres::default().with_tag("18.3-alpine").start().await.unwrap();
    let host = postgres.get_host().await.unwrap();
    let port = postgres.get_host_port_ipv4(5432).await.unwrap();
    let admin = sqlx::PgPool::connect(&format!("postgres://postgres:postgres@{host}:{port}/postgres"))
        .await
        .unwrap();
    sqlx::raw_sql("CREATE ROLE query_role LOGIN PASSWORD 'query-only'; CREATE TABLE public.customers (id integer); INSERT INTO public.customers VALUES (1); GRANT USAGE ON SCHEMA public TO query_role; GRANT SELECT ON public.customers TO query_role;")
        .execute(&admin).await.unwrap();
    let dsn = format!("postgres://query_role:query-only@{host}:{port}/postgres");
    let config = RuntimeConfig::from_input(
        toml::from_str::<ConfigInput>(include_str!("../examples/config.toml")).unwrap(),
        |key| {
            Some(if key.ends_with("DSN") {
                dsn.clone()
            } else {
                "test-token".into()
            })
        },
    )
    .unwrap();
    let permissions = Arc::new(Permissions::default());
    let service = QueryService::connect(config, permissions.clone()).await.unwrap();
    let first = TestIssuer::new().await;
    let second = TestIssuer::new().await;
    let mut authentication = first.authentication();
    authentication.issuers[0]
        .users
        .insert("test-user".into(), "allowed-user".into());
    authentication.issuers[0].clients = [
        ("test-client", "allowed-client"),
        ("agent", "allowed-client"),
        ("denied-agent", "denied-client"),
        ("history", "history-client"),
    ]
    .into_iter()
    .map(|(external, local)| (external.into(), local.into()))
    .collect();
    let mut other = second.authentication();
    other.issuers[0].users.insert("test-user".into(), "other-user".into());
    other.issuers[0]
        .clients
        .insert("test-client".into(), "other-client".into());
    authentication.issuers.extend(other.issuers);
    let authenticator = Arc::new(Authenticator::new(authentication).await.unwrap());
    let other_identity = authenticator.authenticate(&second.token()).await.unwrap();
    assert_eq!(other_identity.user().as_str(), "other-user");
    assert_eq!(other_identity.client().as_str(), "other-client");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bind = listener.local_addr().unwrap();
    let url = format!("http://{bind}/mcp");
    let http = HttpTransportConfig::new(HttpTransportConfigInput {
        bind,
        resource_url: url.clone(),
        allowed_origins: vec![],
    })
    .unwrap();
    let router = http_router(service, authenticator, http);
    let _server = Server(tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    }));
    let client = reqwest::Client::new();
    let arguments = json!({"connection":"analytics-readonly","sql":"SELECT id FROM public.customers"});
    let allowed_token = first.token();
    let denied_token = second.token();
    let (allowed_response, denied_response) = tokio::join!(
        call(&client, &url, &allowed_token, arguments.clone(), json!({})),
        call(
            &client,
            &url,
            &denied_token,
            arguments.clone(),
            json!({"user":"allowed-user","client":"allowed-client"})
        ),
    );
    success(allowed_response).await;
    denied(denied_response).await;
    let mut claims = first.claims();
    claims["act"] = json!({"sub":"agent","iss":first.issuer,"act":{"sub":"history"}});
    success(
        call(
            &client,
            &url,
            &first.token_with_claims(claims),
            arguments.clone(),
            json!({}),
        )
        .await,
    )
    .await;
    let mut claims = first.claims();
    claims["act"] = json!({"sub":"denied-agent","act":{"sub":"agent"}});
    denied(
        call(
            &client,
            &url,
            &first.token_with_claims(claims),
            arguments.clone(),
            json!({}),
        )
        .await,
    )
    .await;
    for actor in [json!({"sub":"unknown"}), json!({"sub":"agent","iss":second.issuer})] {
        let mut claims = first.claims();
        claims["act"] = actor;
        let response = call(
            &client,
            &url,
            &first.token_with_claims(claims),
            arguments.clone(),
            json!({}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let mut spoofed_arguments = arguments.clone();
    spoofed_arguments["user"] = json!("allowed-user");
    spoofed_arguments["client"] = json!("allowed-client");
    let rejected = call(&client, &url, &denied_token, spoofed_arguments, json!({})).await;
    assert_eq!(rejected.status(), StatusCode::OK);
    let rejected: Value = rejected.json().await.unwrap();
    assert_eq!(rejected["error"]["code"], -32602);
    assert_eq!(rejected["error"]["message"], "invalid query arguments");
    success(call(&client, &url, &allowed_token, arguments, json!({})).await).await;
    let checked = permissions.checked.lock().unwrap();
    assert!(checked.contains(&("sql_user".into(), "other-user".into())));
    assert!(checked.contains(&("sql_client".into(), "denied-client".into())));
    assert!(!checked.iter().any(|(_, subject)| subject == "history-client"));
}
