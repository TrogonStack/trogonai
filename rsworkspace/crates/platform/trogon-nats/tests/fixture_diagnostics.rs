#![cfg(feature = "test-support")]

use trogon_nats::test_support::JetStreamTestServer;

#[tokio::test]
async fn selected_evidence_excludes_caller_subjects_descriptions_and_payloads() -> Result<(), Box<dyn std::error::Error>>
{
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = async_nats::jetstream::new(client.clone());
    js.create_stream(async_nats::jetstream::stream::Config {
        name: "CALLER_DATA".to_owned(),
        description: Some("sensitive-fixture-description".to_owned()),
        subjects: vec!["sensitive.fixture.subject".to_owned()],
        ..Default::default()
    })
    .await?;
    client
        .publish("sensitive.fixture.subject", "sensitive-fixture-payload".into())
        .await?;
    client.flush().await?;

    let evidence = server.failure_diagnostics().await;
    drop(server);
    assert!(evidence.contains("Server is ready"));
    assert!(evidence.contains("container_id"));
    assert!(!evidence.contains("sensitive"));
    assert!(!evidence.contains("CALLER_DATA"));
    Ok(())
}
