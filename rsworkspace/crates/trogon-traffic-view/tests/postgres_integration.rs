use chrono::{DateTime, Utc};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use trogon_traffic_view::envelope::AuditEnvelope;
use trogon_traffic_view::indexer::postgres::PostgresIndexer;
use trogon_traffic_view::indexer::TrafficIndex;
use trogon_traffic_view::projector::AuditProjector;

const POSTGRES_IMAGE: &str = "postgres";
const POSTGRES_TAG: &str = "16-alpine";

async fn start_postgres() -> (ContainerAsync<GenericImage>, String) {
    let container = GenericImage::new(POSTGRES_IMAGE, POSTGRES_TAG)
        .with_exposed_port(5432.tcp())
        .with_wait_for(WaitFor::message_on_stderr(
            "database system is ready to accept connections",
        ))
        .with_env_var("POSTGRES_USER", "trogon")
        .with_env_var("POSTGRES_PASSWORD", "trogon")
        .with_env_var("POSTGRES_DB", "traffic")
        .start()
        .await
        .expect("start postgres");

    let port = container.get_host_port_ipv4(5432.tcp()).await.expect("port");
    let url = format!("postgres://trogon:trogon@127.0.0.1:{port}/traffic");
    (container, url)
}

fn sample_sts_envelope(index: usize) -> AuditEnvelope {
    AuditEnvelope {
        event_id: format!("evt-{index}"),
        subject: if index.is_multiple_of(3) {
            "mcp.audit.registry.lookup.found".into()
        } else {
            "mcp.audit.sts.success".into()
        },
        payload: serde_json::json!({
            "event_id": format!("evt-{index}"),
            "outcome": if index.is_multiple_of(5) { "deny" } else { "success" },
            "ts": format!("2026-05-27T14:{:02}:00Z", index % 60),
            "tenant": "acme",
            "agent_id": format!("acme/agent-{index}"),
            "wkl": format!("spiffe://acme.local/ns/prod/sa/agent-{index}"),
            "decision_reason": "ok",
            "session_id": format!("sess-{index}"),
            "request": {
                "audience": format!("urn:trogon:a2a:agent:acme:target-{index}"),
                "purpose": "incident.response",
                "scope": "mcp:tools",
                "wkl": format!("spiffe://acme.local/ns/prod/sa/agent-{index}")
            }
        }),
    }
}

#[tokio::test]
async fn postgres_indexer_roundtrip_fifty_events() {
    if std::env::var("TROGON_PG_URL").is_err() {
        eprintln!("skipping postgres integration test: set TROGON_PG_URL to enable");
        return;
    }

    let (_container, database_url) = start_postgres().await;
    let indexer = PostgresIndexer::connect(&database_url).await.expect("connect");
    indexer.migrate().await.expect("migrate");

    for index in 0..50 {
        let envelope = sample_sts_envelope(index);
        indexer.project(envelope).await.expect("project");
    }

    let rows = indexer
        .query(trogon_traffic_view::TrafficQueryFilter {
            tenant: Some("acme".into()),
            agent_id: Some("acme/agent-1".into()),
            since: Some(DateTime::parse_from_rfc3339("2026-05-27T14:00:00Z").unwrap().with_timezone(&Utc)),
            until: Some(DateTime::parse_from_rfc3339("2026-05-27T15:00:00Z").unwrap().with_timezone(&Utc)),
            limit: Some(100),
        })
        .await
        .expect("query");

    assert!(!rows.is_empty());
    assert!(rows.iter().all(|row| row.tenant == "acme"));
}
