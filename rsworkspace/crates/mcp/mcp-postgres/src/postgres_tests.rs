#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::{QueryResult, decode, verify_bindings};
use crate::{Error, constants::MAX_BYTES};
use sqlx::{Connection, PgConnection, Row, postgres::types::Oid};
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{ImageExt, runners::AsyncRunner},
};

#[test]
fn final_result_budget_includes_column_labels_and_json_escaping() {
    let valid = QueryResult {
        columns: vec!["content".into()],
        rows: vec![vec![serde_json::Value::String("small".into())]],
    };
    assert!(valid.check_size().is_ok());
    let oversized_label = QueryResult {
        columns: vec!["x".repeat(MAX_BYTES)],
        rows: Vec::new(),
    };
    assert!(matches!(oversized_label.check_size(), Err(Error::Limit)));
    let escaped = QueryResult {
        columns: vec!["content".into()],
        rows: vec![vec![serde_json::Value::String("\u{1}".repeat(MAX_BYTES / 4))]],
    };
    assert!(matches!(escaped.check_size(), Err(Error::Limit)));
}

#[tokio::test]
async fn cursor_metadata_and_decoder_reject_incompatible_server_shapes() {
    let container = Postgres::default().with_tag("18.3-alpine").start().await.unwrap();
    let host = container.get_host().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let mut db = PgConnection::connect(&format!("postgres://postgres:postgres@{host}:{port}/postgres"))
        .await
        .unwrap();
    let row = sqlx::query("SELECT false AS overflow, 1::float8 AS unexpected")
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert!(matches!(decode(&row, 1), Err(Error::UnsupportedObject)));
    assert!(matches!(
        verify_bindings(row.columns(), 1, &[Oid(123)]),
        Err(Error::UnsupportedObject)
    ));
    let row = sqlx::query("SELECT false AS overflow, 1 AS visible, 123::oid AS spoofed_relation")
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert!(matches!(
        verify_bindings(row.columns(), 1, &[Oid(123)]),
        Err(Error::UnsupportedObject)
    ));
}
