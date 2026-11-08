use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::core::Mount;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};
use trogon_std::env::InMemoryEnv;

use super::{PostgresSchedulesProjection, PostgresUrl};

pub async fn start_server() -> (ContainerAsync<Postgres>, String) {
    // These disposable projection tests do not exercise crash durability. Keep
    // initdb fsync independent of the shared container daemon's disk contention.
    let container = Postgres::default()
        .with_mount(Mount::tmpfs_mount("/var/lib/postgresql/data").with_size_bytes(256 * 1024 * 1024))
        .start()
        .await
        .expect("start postgres container");
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
