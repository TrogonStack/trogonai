#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
#![cfg(feature = "postgres")]

use std::time::Duration;

use buffa::MessageField;
use trogon_decider_runtime::CommandExecution;
use trogon_scheduler::{
    CreateSchedule, GetScheduleCommand, ListSchedulesCommand, PauseSchedule, PostgresSchedulesProjection,
    RemoveSchedule, ResumeSchedule, ScheduleEventStatus, ScheduleId, SchedulesProjector,
    commands::domain as command_domain, connect_store, projection_queries, projections_v1,
};

#[path = "support/events.rs"]
mod events;
#[path = "support/nats.rs"]
mod nats;
#[path = "support/postgres.rs"]
mod postgres;

fn fixture_schedule_id(label: &str) -> String {
    match label {
        "orders" => "00000000000000000000000000000001",
        "reports" => "00000000000000000000000000000002",
        "ghost" => "00000000000000000000000000000003",
        _ => panic!("missing explicit schedule ID fixture for {label}"),
    }
    .to_string()
}

/// A complete-but-event-less projection, used to seed an orphan row.
fn orphan_projection(id: &str) -> projections_v1::ScheduleProjection {
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

fn base_schedule(id: &str) -> CreateSchedule {
    CreateSchedule {
        id: command_domain::ScheduleId::parse(&fixture_schedule_id(id)).unwrap(),
        status: command_domain::ScheduleEventStatus::Scheduled,
        schedule: command_domain::Schedule::every(Duration::from_secs(2)).unwrap(),
        delivery: command_domain::Delivery::NatsEvent {
            route: command_domain::DeliveryRoute::new("agent.run").unwrap(),
            ttl: Some(command_domain::TtlDuration::from_secs(30).unwrap()),
            source: None,
        },
        message: command_domain::ScheduleMessage {
            content: command_domain::MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
            headers: command_domain::ScheduleHeaders::default(),
        },
    }
}

#[tokio::test]
async fn projector_folds_event_stream_into_postgres() {
    let (_pg_container, pg) = postgres::start().await;
    let (_nats_container, client) = nats::start().await;

    // Append two schedules through the real (NATS-backed) event store.
    let store = connect_store(client.clone()).await.expect("connect store");
    let js = async_nats::jetstream::new(client);
    let projector = SchedulesProjector::new(pg.clone());
    projector.catch_up(&js).await.expect("catch up empty stream");
    assert_eq!(pg.read_checkpoint().await.unwrap(), 0);

    CommandExecution::new(&store.event_store, &base_schedule("orders"))
        .execute()
        .await
        .expect("create orders");
    CommandExecution::new(&store.event_store, &base_schedule("reports"))
        .execute()
        .await
        .expect("create reports");

    // Fold the event stream into Postgres.
    projector.catch_up(&js).await.expect("projector catch up");

    // The Postgres-backed read model now serves both schedules.
    let listed = projection_queries::list_schedules(&pg, ListSchedulesCommand)
        .await
        .expect("list");
    assert_eq!(listed.len(), 2, "both schedules folded into postgres: {listed:?}");

    for id in ["orders", "reports"] {
        assert!(
            projection_queries::get_schedule(
                &pg,
                GetScheduleCommand::new(ScheduleId::parse(&fixture_schedule_id(id)).unwrap())
            )
            .await
            .unwrap()
            .is_some(),
            "postgres projection is missing {id}"
        );
    }

    // Re-running is idempotent: already-checkpointed events are not re-folded.
    projector.catch_up(&js).await.expect("projector catch up again");
    assert_eq!(
        projection_queries::list_schedules(&pg, ListSchedulesCommand)
            .await
            .unwrap()
            .len(),
        2
    );

    let checkpoint = pg.read_checkpoint().await.unwrap();
    let orders_id = command_domain::ScheduleId::parse(&fixture_schedule_id("orders")).unwrap();
    CommandExecution::new(&store.event_store, &PauseSchedule::new(orders_id.clone()))
        .execute()
        .await
        .expect("pause orders");
    projector
        .catch_up(&js)
        .await
        .expect("resume projection from checkpoint");
    let orders = projection_queries::get_schedule(
        &pg,
        GetScheduleCommand::new(ScheduleId::parse(&fixture_schedule_id("orders")).unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(orders.status, ScheduleEventStatus::Paused);
    assert!(pg.read_checkpoint().await.unwrap() > checkpoint);

    CommandExecution::new(
        &store.event_store,
        &RemoveSchedule::new(command_domain::ScheduleId::parse(&fixture_schedule_id("reports")).unwrap()),
    )
    .execute()
    .await
    .expect("remove reports");
    projector.catch_up(&js).await.expect("project removal");
    assert!(
        pg.get_projection(&ScheduleId::parse(&fixture_schedule_id("reports")).unwrap())
            .await
            .unwrap()
            .is_none()
    );

    let mut tail = tokio::task::JoinSet::new();
    let live_projector = projector.clone();
    let live_js = js.clone();
    tail.spawn(async move { live_projector.run(&live_js).await });
    CommandExecution::new(&store.event_store, &ResumeSchedule::new(orders_id))
        .execute()
        .await
        .expect("resume orders");
    let target = js
        .get_stream(trogon_scheduler::constants::EVENTS_STREAM)
        .await
        .unwrap()
        .get_info()
        .await
        .unwrap()
        .state
        .last_sequence;
    tokio::time::timeout(Duration::from_secs(10), async {
        while pg.read_checkpoint().await.unwrap() < target {
            tokio::select! {
                result = tail.join_next() => panic!("live projector exited before checkpointing the event: {result:?}"),
                () = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
        }
    })
    .await
    .expect("live projector reaches the event tail");
    let orders = projection_queries::get_schedule(
        &pg,
        GetScheduleCommand::new(ScheduleId::parse(&fixture_schedule_id("orders")).unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(orders.status, ScheduleEventStatus::Scheduled);
    tail.shutdown().await;

    // A stale row with no backing events plus a reset checkpoint: a from-zero
    // catch-up must reconcile it away.
    pg.upsert_projection(&orphan_projection("ghost"))
        .await
        .expect("seed orphan");
    pg.write_checkpoint(0).await.expect("reset checkpoint");
    projector.catch_up(&js).await.expect("rebuild from zero");

    let ids: Vec<String> = projection_queries::list_schedules(&pg, ListSchedulesCommand)
        .await
        .unwrap()
        .into_iter()
        .map(|schedule| schedule.id)
        .collect();
    assert!(
        !ids.contains(&fixture_schedule_id("ghost")),
        "orphan must be reconciled away: {ids:?}"
    );
    assert_eq!(
        ids,
        vec![fixture_schedule_id("orders")],
        "removed schedules stay absent after replay"
    );
}

#[tokio::test]
async fn malformed_delivery_does_not_block_later_valid_events_or_checkpointing() {
    let (_pg_container, pg) = postgres::start().await;
    let (_nats_container, client) = nats::start().await;
    let store = connect_store(client.clone()).await.unwrap();
    let js = async_nats::jetstream::new(client);
    let projector = SchedulesProjector::new(pg.clone());

    for label in ["orders", "reports"] {
        events::publish_anomalies(&js, &fixture_schedule_id("ghost"), &fixture_schedule_id(label)).await;
        js.publish(
            format!(
                "{}{}",
                trogon_scheduler::constants::EVENTS_SUBJECT_PREFIX,
                fixture_schedule_id("ghost")
            ),
            bytes::Bytes::from_static(b"invalid recorded event"),
        )
        .await
        .unwrap()
        .await
        .unwrap();
        CommandExecution::new(&store.event_store, &base_schedule(label))
            .execute()
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), projector.catch_up(&js))
            .await
            .expect("malformed event must not wedge catch-up")
            .unwrap();
        let target = js
            .get_stream(trogon_scheduler::constants::EVENTS_STREAM)
            .await
            .unwrap()
            .get_info()
            .await
            .unwrap()
            .state
            .last_sequence;
        assert_eq!(pg.read_checkpoint().await.unwrap(), target);
        assert!(
            pg.get_projection(&ScheduleId::parse(&fixture_schedule_id(label)).unwrap())
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            pg.get_projection(&ScheduleId::parse(&fixture_schedule_id("ghost")).unwrap())
                .await
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(pg.list_projections().await.unwrap().len(), 2);
}

#[tokio::test]
async fn database_failure_leaves_the_event_available_for_retry() {
    let (_pg_container, pg) = postgres::start().await;
    let (_nats_container, client) = nats::start().await;
    let store = connect_store(client.clone()).await.unwrap();
    let js = async_nats::jetstream::new(client);
    let projector = SchedulesProjector::new(pg.clone());
    CommandExecution::new(&store.event_store, &base_schedule("orders"))
        .execute()
        .await
        .unwrap();
    sqlx::query("ALTER TABLE schedules_projection RENAME TO unavailable_projection")
        .execute(pg.pool())
        .await
        .unwrap();
    let error = projector
        .catch_up(&js)
        .await
        .expect_err("rebuild requires writable projection storage");
    assert!(std::error::Error::source(&error).is_some());
    assert_eq!(pg.read_checkpoint().await.unwrap(), 0);
    sqlx::query("ALTER TABLE unavailable_projection RENAME TO schedules_projection")
        .execute(pg.pool())
        .await
        .unwrap();
    projector.catch_up(&js).await.unwrap();
    let checkpoint = pg.read_checkpoint().await.unwrap();
    CommandExecution::new(
        &store.event_store,
        &PauseSchedule::new(command_domain::ScheduleId::parse(&fixture_schedule_id("orders")).unwrap()),
    )
    .execute()
    .await
    .unwrap();

    sqlx::query("ALTER TABLE schedules_projection RENAME TO unavailable_projection")
        .execute(pg.pool())
        .await
        .unwrap();
    let error = tokio::time::timeout(Duration::from_secs(10), projector.run(&js))
        .await
        .expect("database failure stops the live projector")
        .expect_err("missing projection table");
    assert!(std::error::Error::source(&error).is_some());
    assert_eq!(pg.read_checkpoint().await.unwrap(), checkpoint);

    sqlx::query("ALTER TABLE unavailable_projection RENAME TO schedules_projection")
        .execute(pg.pool())
        .await
        .unwrap();
    projector.catch_up(&js).await.expect("retry the uncheckpointed event");
    let orders = projection_queries::get_schedule(
        &pg,
        GetScheduleCommand::new(ScheduleId::parse(&fixture_schedule_id("orders")).unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(orders.status, ScheduleEventStatus::Paused);
    assert!(pg.read_checkpoint().await.unwrap() > checkpoint);
}
