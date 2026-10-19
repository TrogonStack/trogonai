#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sqlx::{Connection, Executor, PgConnection, Row, SqlSafeStr, Statement};
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{ImageExt, runners::AsyncRunner},
};

#[tokio::test]
async fn cursor_metadata_binds_original_relation_before_namespace_changes() {
    let container = Postgres::default().with_tag("18.3-alpine").start().await.unwrap();
    let host = container.get_host().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let mut admin = PgConnection::connect(&format!("postgres://postgres:postgres@{host}:{port}/postgres"))
        .await
        .unwrap();
    admin
        .execute("CREATE ROLE query_role LOGIN PASSWORD 'query-only'")
        .await
        .unwrap();
    admin
        .execute("CREATE TABLE public.customers (id integer)")
        .await
        .unwrap();
    admin.execute("INSERT INTO public.customers VALUES (1)").await.unwrap();
    admin
        .execute("GRANT SELECT ON public.customers TO query_role")
        .await
        .unwrap();
    let expected: u32 = sqlx::query_scalar::<_, i64>("SELECT 'public.customers'::regclass::oid::bigint")
        .fetch_one(&mut admin)
        .await
        .unwrap()
        .try_into()
        .unwrap();
    let mut reader = PgConnection::connect(&format!("postgres://query_role:query-only@{host}:{port}/postgres"))
        .await
        .unwrap();
    let mut tx = reader.begin().await.unwrap();
    tx.execute("SET TRANSACTION READ ONLY").await.unwrap();
    sqlx::query("DECLARE mcp_query NO SCROLL CURSOR WITHOUT HOLD FOR SELECT id, customers.tableoid FROM public.customers WHERE id=$1")
        .bind(1_i32).execute(&mut *tx).await.unwrap();
    let metadata = (&mut *tx)
        .prepare("FETCH FORWARD 0 FROM mcp_query".into_sql_str())
        .await
        .unwrap();
    assert_eq!(metadata.columns()[1].relation_id().unwrap().0, expected);
    assert_eq!(metadata.columns()[1].relation_attribute_no(), Some(-6));
    admin.execute("ALTER SCHEMA public RENAME TO previous").await.unwrap();
    admin.execute("CREATE SCHEMA public").await.unwrap();
    admin
        .execute("CREATE TABLE public.customers (id integer)")
        .await
        .unwrap();
    admin.execute("INSERT INTO public.customers VALUES (9)").await.unwrap();
    let rows = sqlx::query("FETCH FORWARD ALL FROM mcp_query")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<i32, _>(0), 1);
    assert_eq!(rows[0].columns()[1].relation_id().unwrap().0, expected);
}
