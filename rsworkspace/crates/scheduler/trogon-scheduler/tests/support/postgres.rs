use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use trogon_std::env::InMemoryEnv;

use super::{PostgresSchedulesProjection, PostgresUrl};

pub async fn start_server() -> (ContainerAsync<Postgres>, String) {
    let container = Postgres::default().start().await.expect("start postgres container");
    let host = container.get_host().await.expect("container host");
    let port = container.get_host_port_ipv4(5432).await.expect("container port");
    let address = format!("postgres://postgres:postgres@{host}:{port}/postgres");
    (container, address)
}

pub async fn start() -> (ContainerAsync<Postgres>, PostgresSchedulesProjection) {
    let (container, address) = start_server().await;
    let url =
        PostgresUrl::parse(&format!("{address}?sslmode=disable"), &InMemoryEnv::new()).expect("test container URL");
    let store = PostgresSchedulesProjection::connect(&url, sqlx::postgres::PgPoolOptions::new().max_connections(5))
        .await
        .expect("connect and run migrations");
    (container, store)
}
