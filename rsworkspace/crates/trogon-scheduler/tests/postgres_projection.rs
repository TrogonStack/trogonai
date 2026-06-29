#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
// Exercises the Postgres schedules read-model backend against a real Postgres in a
// throwaway container. `#[ignore]`d because it needs Docker; run with
// `cargo test -p trogon-scheduler --features postgres -- --ignored`.
#![cfg(all(feature = "postgres", not(coverage)))]

use std::collections::HashSet;

use buffa::MessageField;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use trogon_scheduler::{
    GetScheduleCommand, ListSchedulesCommand, PostgresSchedulesProjection, ScheduleId, SchedulesProjectionStore,
    projection_queries, projections_v1,
};

/// A complete view so the query side's `schedule_from_view` decodes it cleanly: it
/// needs schedule, delivery, and message present.
fn view(id: &str) -> projections_v1::ScheduleProjection {
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
    let projection = PostgresSchedulesProjection::connect(&url)
        .await
        .expect("connect pool and run migrations");
    (container, projection)
}

#[tokio::test]
#[ignore = "requires Docker for testcontainers Postgres"]
async fn upsert_get_list_delete_round_trip() {
    let (_container, projection) = start().await;

    assert!(projection.get_view("missing").await.unwrap().is_none());

    projection.upsert_view(&view("alpha")).await.unwrap();
    projection.upsert_view(&view("beta")).await.unwrap();

    let alpha = projection.get_view("alpha").await.unwrap().expect("alpha present");
    assert_eq!(alpha.schedule_id, "alpha");

    let mut ids: Vec<String> = projection
        .list_views()
        .await
        .unwrap()
        .into_iter()
        .map(|view| view.schedule_id)
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["alpha".to_string(), "beta".to_string()]);

    // Upsert is idempotent: replacing an existing row keeps a single entry.
    projection.upsert_view(&view("alpha")).await.unwrap();
    assert_eq!(projection.list_views().await.unwrap().len(), 2);

    projection.delete_view("alpha").await.unwrap();
    assert!(projection.get_view("alpha").await.unwrap().is_none());
    // Deleting an absent row is a no-op.
    projection.delete_view("alpha").await.unwrap();
}

#[tokio::test]
#[ignore = "requires Docker for testcontainers Postgres"]
async fn reconcile_removes_rows_absent_from_the_live_set() {
    let (_container, projection) = start().await;
    projection.upsert_view(&view("keep")).await.unwrap();
    projection.upsert_view(&view("stale")).await.unwrap();

    projection
        .reconcile(&HashSet::from(["keep".to_string()]))
        .await
        .unwrap();
    assert!(projection.get_view("keep").await.unwrap().is_some());
    assert!(projection.get_view("stale").await.unwrap().is_none());

    // An empty live set clears the table.
    projection.reconcile(&HashSet::new()).await.unwrap();
    assert!(projection.list_views().await.unwrap().is_empty());
}

#[tokio::test]
#[ignore = "requires Docker for testcontainers Postgres"]
async fn checkpoint_round_trips() {
    let (_container, projection) = start().await;

    assert_eq!(projection.read_checkpoint().await.unwrap(), 0);
    projection.write_checkpoint(42).await.unwrap();
    assert_eq!(projection.read_checkpoint().await.unwrap(), 42);
    projection.write_checkpoint(100).await.unwrap();
    assert_eq!(projection.read_checkpoint().await.unwrap(), 100);
}

#[tokio::test]
#[ignore = "requires Docker for testcontainers Postgres"]
async fn schedule_fields_are_stored_as_typed_columns() {
    use sqlx::Row;

    let (_container, projection) = start().await;
    projection.upsert_view(&view("orders")).await.unwrap();

    // Reach past the trait into the raw columns: the schedule's fields are real,
    // queryable columns, not an opaque blob.
    let row = sqlx::query(
        "SELECT schedule_kind, delivery_kind, delivery_subject FROM schedules_projection WHERE schedule_id = $1",
    )
    .bind("orders")
    .fetch_one(projection.pool())
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
    let (_container, projection) = start().await;

    let bytes = vec![0xff, 0xfe, 0x00, 0x01, 0x80];
    let mut view = view("binary");
    view.message = MessageField::some(projections_v1::Message {
        content: MessageField::some(trogonai_proto::content::v1alpha1::Content {
            content_type: "application/octet-stream".to_string(),
            data: bytes.clone(),
        }),
        headers: Vec::new(),
    });
    projection.upsert_view(&view).await.unwrap();

    let stored = projection.get_view("binary").await.unwrap().expect("binary present");
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
    let (_container, projection) = start().await;

    // A cron row whose required cron_expr is missing: it must surface as an error,
    // not be returned as a defaulted (wrong) schedule.
    sqlx::query(
        "INSERT INTO schedules_projection (schedule_id, status, schedule_kind, delivery_kind) \
         VALUES ('broken', 'scheduled', 'cron', 'nats_message')",
    )
    .execute(projection.pool())
    .await
    .unwrap();

    assert!(
        projection.get_view("broken").await.is_err(),
        "a corrupt row must be unreadable, not repaired"
    );

    // And a corrupt row must not suppress the readable ones in a listing.
    projection.upsert_view(&view("ok")).await.unwrap();
    let ids: Vec<String> = projection
        .list_views()
        .await
        .unwrap()
        .into_iter()
        .map(|view| view.schedule_id)
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
    let (_container, projection) = start().await;
    projection.upsert_view(&view("orders")).await.unwrap();

    let fetched = projection_queries::get_schedule(
        &projection,
        GetScheduleCommand::new(ScheduleId::parse("orders").unwrap()),
    )
    .await
    .unwrap()
    .expect("orders present");
    assert_eq!(fetched.id, "orders");

    let listed = projection_queries::list_schedules(&projection, ListSchedulesCommand)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "orders");

    assert!(
        projection_queries::get_schedule(
            &projection,
            GetScheduleCommand::new(ScheduleId::parse("absent").unwrap())
        )
        .await
        .unwrap()
        .is_none()
    );
}
