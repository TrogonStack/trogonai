#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
#![cfg(feature = "postgres")]

use std::collections::HashSet;

use buffa::MessageField;
use buffa_types::google::protobuf::{Duration, Timestamp};
use sqlx::Row;
use trogon_scheduler::{GetScheduleCommand, ListSchedulesCommand, ScheduleId, projection_queries, projections_v1};

fn fixture_schedule_id(label: &str) -> String {
    match label {
        "missing" => "00000000000000000000000000000001",
        "alpha" => "00000000000000000000000000000002",
        "beta" => "00000000000000000000000000000003",
        "keep" => "00000000000000000000000000000004",
        "stale" => "00000000000000000000000000000005",
        "orders" => "00000000000000000000000000000006",
        "binary" => "00000000000000000000000000000007",
        "broken" => "00000000000000000000000000000008",
        "ok" => "00000000000000000000000000000009",
        "absent" => "0000000000000000000000000000000a",
        _ => panic!("missing explicit schedule ID fixture for {label}"),
    }
    .to_string()
}

fn id(raw: &str) -> ScheduleId {
    ScheduleId::parse(&fixture_schedule_id(raw)).unwrap()
}

/// A complete projection so the query side's `schedule_from_view` decodes it
/// cleanly: it needs schedule, delivery, and message present.
fn schedule_projection(id: &str) -> projections_v1::ScheduleProjection {
    projections_v1::ScheduleProjection {
        schedule_id: fixture_schedule_id(id),
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

#[path = "support/postgres.rs"]
mod postgres;

use postgres::start;

#[tokio::test]
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
    assert_eq!(alpha.schedule_id, fixture_schedule_id("alpha"));

    let mut ids: Vec<String> = store
        .list_projections()
        .await
        .unwrap()
        .into_iter()
        .map(|projection| projection.schedule_id)
        .collect();
    ids.sort();
    let mut expected_ids = vec![fixture_schedule_id("alpha"), fixture_schedule_id("beta")];
    expected_ids.sort();
    assert_eq!(ids, expected_ids);

    // Upsert is idempotent: replacing an existing row keeps a single entry.
    store.upsert_projection(&schedule_projection("alpha")).await.unwrap();
    assert_eq!(store.list_projections().await.unwrap().len(), 2);

    store.delete_projection(&id("alpha")).await.unwrap();
    assert!(store.get_projection(&id("alpha")).await.unwrap().is_none());
    // Deleting an absent row is a no-op.
    store.delete_projection(&id("alpha")).await.unwrap();
}

#[tokio::test]
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
async fn checkpoint_round_trips() {
    let (_container, store) = start().await;

    assert_eq!(store.read_checkpoint().await.unwrap(), 0);
    store.write_checkpoint(42).await.unwrap();
    assert_eq!(store.read_checkpoint().await.unwrap(), 42);
    store.write_checkpoint(100).await.unwrap();
    assert_eq!(store.read_checkpoint().await.unwrap(), 100);
}

#[tokio::test]
async fn schedule_fields_are_stored_as_typed_columns() {
    let (_container, store) = start().await;
    store.upsert_projection(&schedule_projection("orders")).await.unwrap();

    // Reach past the trait into the raw columns: the schedule's fields are real,
    // queryable columns, not an opaque blob.
    let row = sqlx::query(
        "SELECT schedule_kind, delivery_kind, delivery_subject FROM schedules_projection WHERE schedule_id = $1",
    )
    .bind(fixture_schedule_id("orders"))
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
async fn corrupt_row_is_unreadable_not_silently_repaired() {
    let (_container, store) = start().await;

    // A fully schema-valid row (every constraint satisfied) whose JSONB headers are
    // malformed because a header name is not a string, so the corruption is purely at the
    // application-decode layer and does not depend on any column's nullability. It
    // must surface as an error, not be returned as a defaulted (wrong) schedule.
    sqlx::query(
        "INSERT INTO schedules_projection \
             (schedule_id, status, schedule_kind, cron_expr, delivery_kind, delivery_subject, message_headers) \
         VALUES ($1, 'scheduled', 'cron', '* * * * *', 'nats_message', 'agent.run', '[{\"name\": 5, \"value\": \"x\"}]')",
    )
    .bind(fixture_schedule_id("broken"))
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
        vec![fixture_schedule_id("ok")],
        "list skips the corrupt row, keeps the rest"
    );
}

#[tokio::test]
async fn projection_queries_read_through_the_backend() {
    let (_container, store) = start().await;
    store.upsert_projection(&schedule_projection("orders")).await.unwrap();

    let fetched = projection_queries::get_schedule(&store, GetScheduleCommand::new(id("orders")))
        .await
        .unwrap()
        .expect("orders present");
    assert_eq!(fetched.id, fixture_schedule_id("orders"));

    let listed = projection_queries::list_schedules(&store, ListSchedulesCommand)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, fixture_schedule_id("orders"));

    assert!(
        projection_queries::get_schedule(&store, GetScheduleCommand::new(id("absent")))
            .await
            .unwrap()
            .is_none()
    );
}

fn timestamp(seconds: i64) -> Timestamp {
    Timestamp {
        seconds,
        nanos: 123_456_000,
        ..Default::default()
    }
}

#[tokio::test]
async fn replacing_schedule_variants_preserves_the_complete_projection() {
    let (_container, store) = start().await;
    let timezone = MessageField::some(trogonai_proto::google::r#type::TimeZone {
        id: "America/New_York".to_string(),
        ..Default::default()
    });
    let definitions = [
        projections_v1::Schedule {
            kind: Some(
                projections_v1::schedule::At {
                    at: MessageField::some(timestamp(1_800_000_000)),
                }
                .into(),
            ),
        },
        projections_v1::Schedule {
            kind: Some(
                projections_v1::schedule::Cron {
                    expr: "0 9 * * *".to_string(),
                    timezone: timezone.clone(),
                }
                .into(),
            ),
        },
        projections_v1::Schedule {
            kind: Some(
                projections_v1::schedule::RRule {
                    dtstart: MessageField::some(timestamp(1_800_000_000)),
                    rrule: "FREQ=DAILY;COUNT=5".to_string(),
                    timezone,
                    rdate: vec![timestamp(1_800_086_400), timestamp(1_800_172_800)],
                    exdate: vec![timestamp(1_800_259_200)],
                }
                .into(),
            ),
        },
        projections_v1::Schedule {
            kind: Some(
                projections_v1::schedule::Every {
                    every: MessageField::some(Duration {
                        seconds: 30,
                        ..Default::default()
                    }),
                }
                .into(),
            ),
        },
    ];
    let mut projection = schedule_projection("orders");
    projection.status = MessageField::some(projections_v1::ScheduleStatus {
        kind: Some(projections_v1::schedule_status::Paused {}.into()),
    });
    projection.completed = Some(true);
    projection.next_occurrence_at = MessageField::some(timestamp(1_800_086_400));
    projection.last_occurrence_at = MessageField::some(timestamp(1_800_000_000));
    projection.delivery = MessageField::some(projections_v1::Delivery {
        kind: Some(
            projections_v1::delivery::NatsMessage {
                subject: "agent.run".to_string(),
                ttl: MessageField::some(Duration {
                    seconds: 60,
                    ..Default::default()
                }),
                source: MessageField::some(projections_v1::delivery::nats_message::Source {
                    kind: Some(
                        projections_v1::delivery::nats_message::LatestFromSubject {
                            subject: "agent.input".to_string(),
                        }
                        .into(),
                    ),
                }),
            }
            .into(),
        ),
    });
    projection.message = MessageField::some(projections_v1::Message {
        content: MessageField::some(trogonai_proto::content::v1alpha1::Content {
            content_type: "application/json".to_string(),
            data: br#"{"kind":"heartbeat"}"#.to_vec(),
        }),
        headers: vec![
            projections_v1::Header {
                name: "X-Category".to_string(),
                value: "orders".to_string(),
            },
            projections_v1::Header {
                name: "X-Category".to_string(),
                value: "reports".to_string(),
            },
        ],
    });

    for definition in definitions {
        projection.schedule = MessageField::some(definition);
        store.upsert_projection(&projection).await.unwrap();
        assert_eq!(
            store.get_projection(&id("orders")).await.unwrap(),
            Some(projection.clone())
        );
        assert_eq!(store.list_projections().await.unwrap(), vec![projection.clone()]);
    }

    let row = sqlx::query(
        "SELECT at_at, cron_expr, rrule, rrule_dtstart, timezone, cardinality(rrule_rdate) AS rdates, \
         cardinality(rrule_exdate) AS exdates FROM schedules_projection WHERE schedule_id = $1",
    )
    .bind(fixture_schedule_id("orders"))
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert!(row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("at_at").is_none());
    assert!(row.get::<Option<String>, _>("cron_expr").is_none());
    assert!(row.get::<Option<String>, _>("rrule").is_none());
    assert!(
        row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("rrule_dtstart")
            .is_none()
    );
    assert!(row.get::<Option<String>, _>("timezone").is_none());
    assert_eq!(row.get::<i32, _>("rdates"), 0);
    assert_eq!(row.get::<i32, _>("exdates"), 0);
}

#[tokio::test]
async fn invalid_projection_input_does_not_overwrite_a_valid_row() {
    let (_container, store) = start().await;
    let projection = schedule_projection("orders");
    store.upsert_projection(&projection).await.unwrap();
    let original = store.get_projection(&id("orders")).await.unwrap();

    let mut missing_schedule = projection.clone();
    missing_schedule.schedule = MessageField::none();
    let mut missing_schedule_kind = projection.clone();
    missing_schedule_kind.schedule = MessageField::some(projections_v1::Schedule::default());
    let mut missing_delivery = projection.clone();
    missing_delivery.delivery = MessageField::none();
    let mut missing_delivery_kind = projection.clone();
    missing_delivery_kind.delivery = MessageField::some(projections_v1::Delivery::default());
    let mut missing_message = projection.clone();
    missing_message.message = MessageField::none();
    let mut invalid_timestamp = projection.clone();
    invalid_timestamp.next_occurrence_at = MessageField::some(timestamp(i64::MAX));

    for invalid in [
        missing_schedule,
        missing_schedule_kind,
        missing_delivery,
        missing_delivery_kind,
        missing_message,
        invalid_timestamp,
    ] {
        assert!(store.upsert_projection(&invalid).await.is_err());
        assert_eq!(store.get_projection(&id("orders")).await.unwrap(), original);
    }
}

#[tokio::test]
async fn incomplete_columns_and_malformed_headers_do_not_hide_valid_rows() {
    let (_container, store) = start().await;
    store.upsert_projection(&schedule_projection("ok")).await.unwrap();
    for corruption in [
        "UPDATE schedules_projection SET schedule_kind = 'cron', cron_expr = NULL WHERE schedule_id = $1",
        "UPDATE schedules_projection SET schedule_kind = 'rrule', rrule = NULL WHERE schedule_id = $1",
        "UPDATE schedules_projection SET delivery_subject = NULL WHERE schedule_id = $1",
        "UPDATE schedules_projection SET message_content_type = 'application/json', message_body = NULL WHERE schedule_id = $1",
        "UPDATE schedules_projection SET message_headers = '{}'::jsonb WHERE schedule_id = $1",
        "UPDATE schedules_projection SET message_headers = '[{\"name\":\"X-Category\",\"value\":5}]'::jsonb WHERE schedule_id = $1",
    ] {
        store.upsert_projection(&schedule_projection("broken")).await.unwrap();
        sqlx::query(corruption)
            .bind(fixture_schedule_id("broken"))
            .execute(store.pool())
            .await
            .unwrap();
        assert!(store.get_projection(&id("broken")).await.is_err(), "{corruption}");
        let listed = store.list_projections().await.unwrap();
        assert_eq!(listed.len(), 1, "{corruption}");
        assert_eq!(listed[0].schedule_id, fixture_schedule_id("ok"));
    }
}
