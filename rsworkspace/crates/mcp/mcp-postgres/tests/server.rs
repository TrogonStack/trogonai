#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use async_trait::async_trait;
use mcp_postgres::{ConfigInput, Error, PermissionCheck, PermissionRequest, PostgresMcp, QueryService, RuntimeConfig};
use rmcp::{ServiceExt, model::CallToolRequestParams};
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
    let (client_io, server_io) = tokio::io::duplex(65536);
    let server = tokio::spawn(async move {
        PostgresMcp::new(service)
            .serve(server_io)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let client = ().serve(client_io).await.unwrap();
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
    server.await.unwrap();
    let output = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
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
        assert!(output.contains(expected), "missing {expected}: {output}");
    }
    for secret in ["SELECT", "private-row@example.test", "private-token", &dsn] {
        assert!(!output.contains(secret), "audit leaked {secret}");
    }
}
