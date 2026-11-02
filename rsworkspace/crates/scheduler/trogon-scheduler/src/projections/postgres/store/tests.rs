use sqlx::postgres::PgPoolOptions;
use trogon_std::env::InMemoryEnv;

use super::*;
use crate::PostgresUrl;

#[tokio::test]
async fn an_explicit_tls_disable_reaches_the_connection_attempt() {
    let url = PostgresUrl::parse(
        "postgres://postgres:postgres@127.0.0.1:1/postgres?sslmode=disable",
        &InMemoryEnv::new(),
    )
    .expect("an explicit TLS disable is valid");

    let Err(error) = PostgresSchedulesProjection::connect(
        &url,
        PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_millis(100)),
    )
    .await
    else {
        panic!("the discarded local port has no Postgres server");
    };

    assert!(error.to_string().contains("failed to connect"));
}
