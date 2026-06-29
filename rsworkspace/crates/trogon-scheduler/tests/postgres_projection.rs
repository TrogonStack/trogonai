#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
// Exercises the Postgres schedules read-model backend against a real Postgres in a
// throwaway container. `#[ignore]`d because it needs Docker; run with
// `cargo test -p trogon-scheduler --features postgres -- --ignored`.
#![cfg(all(feature = "postgres", not(coverage)))]

use std::collections::HashSet;

use buffa::MessageField;
use sqlx::Row;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use trogon_scheduler::{
    GetScheduleCommand, ListSchedulesCommand, PostgresSchedulesProjection, ScheduleId, projection_queries,
    projections_v1,
};

fn id(raw: &str) -> ScheduleId {
    ScheduleId::parse(raw).unwrap()
}

/// A complete projection so the query side's `schedule_from_view` decodes it
/// cleanly: it needs schedule, delivery, and message present.
fn schedule_projection(id: &str) -> projections_v1::ScheduleProjection {
    projections_v1::ScheduleProjection {
        schedule_id: id.to_string(),
        schedule: MessageField::some(projections_v1::Schedule {
            kind: Some(
                projections_v1::schedule::Every {
                    every: MessageField::none(),
                }
                .into(),
            ),
        }),
        delivery: MessageField::some(projections_v1::Delivery {
            kind: Some(
                projections_v1::delivery::NatsMessage {
                    subject: "agent.run".to_string(),
                    ttl: MessageField::none(),
                    source: MessageField::none(),
                }
                .into(),
            ),
        }),
        message: MessageField::some(projections_v1::Message::default()),
        ..Default::default()
    }
}

async fn start() -> (ContainerAsync<Postgres>, PostgresSchedulesProjection) {
    let container = Postgres::default().start().await.expect("start postgres container");
    let host = container.get_host().await.expect("container host");
    let port = container.get_host_port_ipv4(5432).await.expect("container port");
    let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .expect("connect pool");
    let store = PostgresSchedulesProjection::new(pool).await.expect("run migrations");
    (container, store)
}

#[tokio::test]
#[ignore = "requires Docker for testcontainers Postgres"]
async fn upsert_get_list_delete_round_trip() {
    let (_container, store) = start().await;

    assert!(store.get_projection(&id("missing")).await.unwrap().is_none());

    store.upsert_projection(&schedule_projection("alpha")).await.unwrap();
    store.upsert_projection(&schedule_projection("beta")).await.unwrap();

    let alpha = store
        .get_projection(&id("alpha"))
        .await
        .unwrap()
        .expect("alpha present");
    assert_eq!(alpha.schedule_id, "alpha");

    let mut ids: Vec<String> = store
        .list_projections()
        .await
        .unwrap()
        .into_iter()
        .map(|projection| projection.schedule_id)
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["alpha".to_string(), "beta".to_string()]);

    // Upsert is idempotent: replacing an existing row keeps a single entry.
    store.upsert_projection(&schedule_projection("alpha")).await.unwrap();
    assert_eq!(store.list_projections().await.unwrap().len(), 2);

    store.delete_projection(&id("alpha")).await.unwrap();
    assert!(store.get_projection(&id("alpha")).await.unwrap().is_none());
    // Deleting an absent row is a no-op.
    store.delete_projection(&id("alpha")).await.unwrap();
}

#[tokio::test]
#[ignore = "requires Docker for testcontainers Postgres"]
async fn reconcile_removes_rows_absent_from_the_live_set() {
    let (_container, store) = start().await;
    store.upsert_projection(&schedule_projection("keep")).await.unwrap();
    store.upsert_projection(&schedule_projection("stale")).await.unwrap();

    store.reconcile(&HashSet::from([id("keep")])).await.unwrap();
    assert!(store.get_projection(&id("keep")).await.unwrap().is_some());
    assert!(store.get_projection(&id("stale")).await.unwrap().is_none());

    // An empty live set clears the table.
    store.reconcile(&HashSet::new()).await.unwrap();
    assert!(store.list_projections().await.unwrap().is_empty());
}

#[tokio::test]
#[ignore = "requires Docker for testcontainers Postgres"]
async fn checkpoint_round_trips() {
    let (_container, store) = start().await;

    assert_eq!(store.read_checkpoint().await.unwrap(), 0);
    store.write_checkpoint(42).await.unwrap();
    assert_eq!(store.read_checkpoint().await.unwrap(), 42);
    store.write_checkpoint(100).await.unwrap();
    assert_eq!(store.read_checkpoint().await.unwrap(), 100);
}

#[tokio::test]
#[ignore = "requires Docker for testcontainers Postgres"]
async fn schedule_fields_are_stored_as_typed_columns() {
    let (_container, store) = start().await;
    store.upsert_projection(&schedule_projection("orders")).await.unwrap();

    // Reach past the trait into the raw columns: the schedule's fields are real,
    // queryable columns, not an opaque blob.
    let row = sqlx::query(
        "SELECT schedule_kind, delivery_kind, delivery_subject FROM schedules_projection WHERE schedule_id = $1",
    )
    .bind("orders")
    .fetch_one(store.pool())
    .await
    .unwrap();

    let schedule_kind: String = row.get("schedule_kind");
    let delivery_kind: String = row.get("delivery_kind");
    let delivery_subject: Option<String> = row.get("delivery_subject");
    assert_eq!(schedule_kind, "every");
    assert_eq!(delivery_kind, "nats_message");
    assert_eq!(delivery_subject.as_deref(), Some("agent.run"));
}

#[tokio::test]
#[ignore = "requires Docker for testcontainers Postgres"]
async fn non_utf8_message_body_round_trips() {
    let (_container, store) = start().await;

    let bytes = vec![0xff, 0xfe, 0x00, 0x01, 0x80];
    let mut projection = schedule_projection("binary");
    projection.message = MessageField::some(projections_v1::Message {
        content: MessageField::some(trogonai_proto::content::v1alpha1::Content {
            content_type: "application/octet-stream".to_string(),
            data: bytes.clone(),
        }),
        headers: Vec::new(),
    });
    store.upsert_projection(&projection).await.unwrap();

    let stored = store
        .get_projection(&id("binary"))
        .await
        .unwrap()
        .expect("binary present");
    let data = stored
        .message
        .as_option()
        .and_then(|message| message.content.as_option())
        .map(|content| content.data.clone())
        .expect("content present");
    assert_eq!(data, bytes, "non-UTF-8 body must round-trip byte-for-byte");
}

#[tokio::test]
#[ignore = "requires Docker for testcontainers Postgres"]
async fn corrupt_row_is_unreadable_not_silently_repaired() {
    let (_container, store) = start().await;

    // A fully schema-valid row (every constraint satisfied) whose JSONB headers are
    // malformed — a non-string header name — so the corruption is purely at the
    // application-decode layer and does not depend on any column's nullability. It
    // must surface as an error, not be returned as a defaulted (wrong) schedule.
    sqlx::query(
        "INSERT INTO schedules_projection \
             (schedule_id, status, schedule_kind, cron_expr, delivery_kind, delivery_subject, message_headers) \
         VALUES ('broken', 'scheduled', 'cron', '* * * * *', 'nats_message', 'agent.run', '[{\"name\": 5, \"value\": \"x\"}]')",
    )
    .execute(store.pool())
    .await
    .unwrap();

    assert!(
        store.get_projection(&id("broken")).await.is_err(),
        "a corrupt row must be unreadable, not repaired"
    );

    // And a corrupt row must not suppress the readable ones in a listing.
    store.upsert_projection(&schedule_projection("ok")).await.unwrap();
    let ids: Vec<String> = store
        .list_projections()
        .await
        .unwrap()
        .into_iter()
        .map(|projection| projection.schedule_id)
        .collect();
    assert_eq!(
        ids,
        vec!["ok".to_string()],
        "list skips the corrupt row, keeps the rest"
    );
}

#[tokio::test]
#[ignore = "requires Docker for testcontainers Postgres"]
async fn projection_queries_read_through_the_backend() {
    let (_container, store) = start().await;
    store.upsert_projection(&schedule_projection("orders")).await.unwrap();

    let fetched =
        projection_queries::get_schedule(&store, GetScheduleCommand::new(ScheduleId::parse("orders").unwrap()))
            .await
            .unwrap()
            .expect("orders present");
    assert_eq!(fetched.id, "orders");

    let listed = projection_queries::list_schedules(&store, ListSchedulesCommand)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "orders");

    assert!(
        projection_queries::get_schedule(&store, GetScheduleCommand::new(ScheduleId::parse("absent").unwrap()))
            .await
            .unwrap()
            .is_none()
    );
}
