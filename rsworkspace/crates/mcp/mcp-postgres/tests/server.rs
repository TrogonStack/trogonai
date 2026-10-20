#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod common;
use async_trait::async_trait;
use mcp_postgres::{ConfigInput, Error, PermissionCheck, PermissionRequest, QueryService, RuntimeConfig};
use rmcp::model::CallToolRequestParams;
use serde_json::json;
use std::sync::{Arc, Mutex};
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{ImageExt, runners::AsyncRunner},
};
use tracing_subscriber::{Layer, layer::SubscriberExt};
#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}
struct Permissions(Arc<Mutex<Option<Error>>>);
#[async_trait]
impl PermissionCheck for Permissions {
    async fn check_all(&self, _: &[PermissionRequest]) -> Result<(), Error> {
        match self.0.lock().unwrap().take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
#[tokio::test(flavor = "current_thread")]
async fn mcp_errors_are_safe_and_audits_exclude_sql_parameters_and_rows() {
    let container = Postgres::default().with_tag("18.3-alpine").start().await.unwrap();
    let dsn = format!(
        "postgres://postgres:postgres@{}:{}/postgres",
        container.get_host().await.unwrap(),
        container.get_host_port_ipv4(5432).await.unwrap()
    );
    let pool = sqlx::PgPool::connect(&dsn).await.unwrap();
    sqlx::query("CREATE TABLE public.customers (id integer, email text)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO public.customers VALUES (1, 'private-row@example.test')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE ROLE query_role LOGIN PASSWORD 'query-only'; GRANT USAGE ON SCHEMA public TO query_role; GRANT SELECT ON public.customers TO query_role;").execute(&pool).await.unwrap();
    let dsn = dsn.replace("postgres:postgres@", "query_role:query-only@");
    let input: ConfigInput = toml::from_str(include_str!("../examples/config.toml")).unwrap();
    let config = RuntimeConfig::from_input(input, |key| {
        Some(if key.ends_with("DSN") {
            dsn.clone()
        } else {
            "private-token".into()
        })
    })
    .unwrap();
    let next_error = Arc::new(Mutex::new(None));
    let service = QueryService::connect(config, Arc::new(Permissions(next_error.clone())))
        .await
        .unwrap();
    let logs = LogBuffer::default();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .json()
            .with_writer(logs.clone())
            .with_filter(tracing_subscriber::filter::Targets::new().with_target("mcp_postgres", tracing::Level::INFO)),
    );
    let _guard = tracing::subscriber::set_default(subscriber);
    let http = common::HttpFixture::with_origins(service, &["https://portal.example.test"]).await;
    let http_client = reqwest::Client::new();
    let preflight = http_client
        .request(reqwest::Method::OPTIONS, &http.url)
        .header("Origin", "https://portal.example.test")
        .header("Access-Control-Request-Method", "POST")
        .header(
            "Access-Control-Request-Headers",
            "authorization,content-type,mcp-protocol-version",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(preflight.status(), reqwest::StatusCode::OK);
    assert_eq!(
        preflight.headers()["access-control-allow-origin"],
        "https://portal.example.test"
    );
    assert!(
        preflight.headers()["access-control-allow-headers"]
            .to_str()
            .unwrap()
            .contains("authorization")
    );
    let allowed_origin = http_client
        .post(&http.url)
        .header("Origin", "https://portal.example.test")
        .send()
        .await
        .unwrap();
    assert_eq!(allowed_origin.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(
        allowed_origin.headers()["access-control-allow-origin"],
        "https://portal.example.test"
    );
    assert!(
        allowed_origin.headers()["access-control-expose-headers"]
            .to_str()
            .unwrap()
            .contains("www-authenticate")
    );
    let missing = http_client.post(&http.url).send().await.unwrap();
    assert_eq!(missing.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert!(
        missing.headers()["www-authenticate"]
            .to_str()
            .unwrap()
            .contains("/.well-known/oauth-protected-resource/mcp")
    );
    for authorization in ["Basic example", "Bearer", "Bearer bad token", "Bearer invalid"] {
        let denied = http_client
            .post(&http.url)
            .header("Authorization", authorization)
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), reqwest::StatusCode::UNAUTHORIZED);
    }
    let duplicate = http_client
        .post(&http.url)
        .header("Authorization", format!("Bearer {}", http.issuer.token()))
        .header("Authorization", format!("Bearer {}", http.issuer.token()))
        .send()
        .await
        .unwrap();
    assert_eq!(duplicate.status(), reqwest::StatusCode::UNAUTHORIZED);
    let mut claims = http.issuer.claims();
    claims["scope"] = json!("unrelated");
    let insufficient = http_client
        .post(&http.url)
        .bearer_auth(http.issuer.token_with_claims(claims))
        .send()
        .await
        .unwrap();
    assert_eq!(insufficient.status(), reqwest::StatusCode::FORBIDDEN);
    assert!(
        insufficient.headers()["www-authenticate"]
            .to_str()
            .unwrap()
            .contains("error=\"insufficient_scope\"")
    );
    let blocked_origin = http_client
        .post(&http.url)
        .bearer_auth(http.issuer.token())
        .header("Origin", "https://untrusted.example.test")
        .send()
        .await
        .unwrap();
    assert_eq!(blocked_origin.status(), reqwest::StatusCode::FORBIDDEN);
    let blocked_port = http_client
        .post(&http.url)
        .bearer_auth(http.issuer.token())
        .header("Origin", "https://portal.example.test:8443")
        .send()
        .await
        .unwrap();
    assert_eq!(blocked_port.status(), reqwest::StatusCode::FORBIDDEN);
    let blocked_host = http_client
        .post(&http.url)
        .bearer_auth(http.issuer.token())
        .header("Host", "untrusted.example.test")
        .send()
        .await
        .unwrap();
    assert_eq!(blocked_host.status(), reqwest::StatusCode::FORBIDDEN);
    let oversized = http_client
        .post(&http.url)
        .bearer_auth(http.issuer.token())
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body("x".repeat(131_073))
        .send()
        .await
        .unwrap();
    assert_eq!(oversized.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);
    let metadata_url = http.url.replace("/mcp", "/.well-known/oauth-protected-resource/mcp");
    let metadata: serde_json::Value = http_client
        .get(metadata_url)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(metadata["resource"], http.url);
    assert_eq!(metadata["authorization_servers"], json!([http.issuer.issuer]));
    assert_eq!(metadata["bearer_methods_supported"], json!(["header"]));
    let client = http.client().await;
    assert!(client.call_tool(CallToolRequestParams::new("missing")).await.is_err());
    for (connection, failure, expected) in [
        ("missing", None, "unknown connection"),
        ("analytics-readonly", Some(Error::Limit), "resource limit exceeded"),
        ("analytics-readonly", Some(Error::Timeout), "request timed out"),
        ("analytics-readonly", Some(Error::Configuration), "query unavailable"),
    ] {
        *next_error.lock().unwrap() = failure;
        let response = client
            .call_tool(
                CallToolRequestParams::new("query").with_arguments(
                    json!({"connection":connection,"sql":"SELECT email FROM public.customers"})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();
        assert_eq!(response.is_error, Some(true));
        assert_eq!(response.content[0].as_text().unwrap().text, expected);
    }
    let response = client.call_tool(CallToolRequestParams::new("query").with_arguments(json!({"connection":"analytics-readonly","sql":"SELECT email FROM public.customers WHERE email = $1", "parameters":["private-row@example.test"]}).as_object().unwrap().clone())).await.unwrap();
    assert_ne!(response.is_error, Some(true));
    let result: serde_json::Value = serde_json::from_str(&response.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(result["columns"], json!(["email"]));
    assert_eq!(result["rows"], json!([["private-row@example.test"]]));
    client.cancel().await.unwrap();
    let bearer_token = http.token.clone();
    http.issuer.stop().await;
    let unavailable_token = http.issuer.token_with_key_id(http.issuer.claims(), "rotated-key");
    let unavailable = http_client
        .post(&http.url)
        .bearer_auth(unavailable_token)
        .send()
        .await
        .unwrap();
    assert_eq!(unavailable.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert!(unavailable.headers().get("www-authenticate").is_none());
    let output = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert_audit_output(
        &output,
        &[
            "SELECT",
            "private-row@example.test",
            "private-token",
            &dsn,
            &bearer_token,
        ],
    );
}

fn assert_audit_output(output: &str, forbidden: &[&str]) {
    for expected in [
        "sql_query_admission",
        "sql_query_completion",
        "sql_query_reference",
        "Timeout",
        "Unavailable",
        "resource_id",
        "example-user",
        "example-client",
    ] {
        assert!(output.contains(expected), "audit metadata is missing");
    }
    for secret in forbidden {
        assert!(!output.contains(secret), "audit contains a forbidden value");
    }
}

#[test]
fn audit_assertion_failures_do_not_disclose_captured_output_or_forbidden_values() {
    let canary = "audit-privacy-regression-canary";
    let complete_metadata = "sql_query_admission sql_query_completion sql_query_reference Timeout Unavailable resource_id example-user example-client";
    let failures = [canary.to_owned(), format!("{complete_metadata} {canary}")]
        .map(|output| std::panic::catch_unwind(|| assert_audit_output(&output, &[canary])));
    for failure in failures {
        let payload = failure.expect_err("audit assertion accepted invalid output");
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .expect("audit assertion did not provide a text diagnostic");
        assert!(!message.contains(canary), "audit assertion disclosed a forbidden value");
        assert!(
            !message.contains(complete_metadata),
            "audit assertion disclosed captured output"
        );
    }
}
