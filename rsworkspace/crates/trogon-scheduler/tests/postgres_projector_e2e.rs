#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
// End-to-end: append schedules through the real NATS event store, then fold the
// event stream into Postgres with `SchedulesProjector`. Both backends run in
// throwaway containers, so this needs Docker; run with
// `cargo test -p trogon-scheduler --features postgres -- --ignored`.
#![cfg(all(feature = "postgres", not(coverage)))]

use std::time::Duration;

use buffa::MessageField;
use testcontainers_modules::nats::{Nats, NatsServerCmd};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ImageExt;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use trogon_decider_runtime::CommandExecution;
use trogon_scheduler::{
    CreateSchedule, GetScheduleCommand, ListSchedulesCommand, PostgresSchedulesProjection, ScheduleId,
    SchedulesProjectionStore, SchedulesProjector, commands::domain as command_domain, connect_store,
    projection_queries, projections_v1,
};

/// A complete-but-event-less view, used to seed an orphan row.
fn orphan_view(id: &str) -> projections_v1::ScheduleProjection {
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

fn base_schedule(id: &str) -> CreateSchedule {
    CreateSchedule {
        id: command_domain::ScheduleId::parse(id).unwrap(),
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
#[ignore = "requires Docker for testcontainers NATS + Postgres"]
async fn projector_folds_event_stream_into_postgres() {
    // Postgres backend.
    let pg_container = Postgres::default().start().await.expect("start postgres");
    let pg_host = pg_container.get_host().await.expect("postgres host");
    let pg_port = pg_container.get_host_port_ipv4(5432).await.expect("postgres port");
    let pg_url = format!("postgres://postgres:postgres@{pg_host}:{pg_port}/postgres");
    let pg = PostgresSchedulesProjection::connect(&pg_url)
        .await
        .expect("connect postgres + migrate");

    // NATS with JetStream for the event store.
    let nats_cmd = NatsServerCmd::default().with_jetstream();
    // The event store requires NATS 2.12+ (atomic batch publish), so pin a tag that
    // supports it rather than the module's older default.
    let nats_container = Nats::default()
        .with_cmd(&nats_cmd)
        .with_tag("2.14.2-alpine")
        .start()
        .await
        .expect("start nats");
    let nats_host = nats_container.get_host().await.expect("nats host");
    let nats_port = nats_container.get_host_port_ipv4(4222).await.expect("nats port");
    let nats_url = format!("nats://{nats_host}:{nats_port}");
    let client = async_nats::connect(&nats_url).await.expect("connect nats");

    // Append two schedules through the real (NATS-backed) event store.
    let store = connect_store(client.clone()).await.expect("connect store");
    CommandExecution::new(&store.event_store, &base_schedule("orders"))
        .execute()
        .await
        .expect("create orders");
    CommandExecution::new(&store.event_store, &base_schedule("reports"))
        .execute()
        .await
        .expect("create reports");

    // Fold the event stream into Postgres.
    let js = async_nats::jetstream::new(client);
    let projector = SchedulesProjector::new(pg.clone());
    projector.catch_up(&js).await.expect("projector catch up");

    // The Postgres-backed read model now serves both schedules.
    let listed = projection_queries::list_schedules(&pg, ListSchedulesCommand)
        .await
        .expect("list");
    assert_eq!(listed.len(), 2, "both schedules folded into postgres: {listed:?}");

    for id in ["orders", "reports"] {
        assert!(
            projection_queries::get_schedule(&pg, GetScheduleCommand::new(ScheduleId::parse(id).unwrap()))
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

    // A stale row with no backing events plus a reset checkpoint: a from-zero
    // catch-up must reconcile it away.
    pg.upsert_view(&orphan_view("ghost")).await.expect("seed orphan");
    pg.write_checkpoint(0).await.expect("reset checkpoint");
    projector.catch_up(&js).await.expect("rebuild from zero");

    let ids: Vec<String> = projection_queries::list_schedules(&pg, ListSchedulesCommand)
        .await
        .unwrap()
        .into_iter()
        .map(|schedule| schedule.id)
        .collect();
    assert!(
        !ids.contains(&"ghost".to_string()),
        "orphan must be reconciled away: {ids:?}"
    );
    assert_eq!(ids.len(), 2, "the two event-backed schedules survive: {ids:?}");
}
