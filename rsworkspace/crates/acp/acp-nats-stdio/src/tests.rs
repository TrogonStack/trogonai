use super::*;
use trogon_nats::AdvancedMockNatsClient;
use trogon_nats::jetstream::MockJetStreamClient;

#[tokio::test]
async fn run_bridge_shuts_down_on_signal() {
    let mock = AdvancedMockNatsClient::new();
    let _sub = mock.inject_messages();
    let config = acp_nats::Config::new(
        acp_nats::AcpPrefix::new("acp").unwrap(),
        acp_nats::NatsConfig::new(vec!["localhost:4222".to_string()], trogon_nats::NatsAuth::None),
    );

    let (reader, _writer) = tokio::io::duplex(1024);
    let (_reader2, writer2) = tokio::io::duplex(1024);
    let stdin = async_compat::Compat::new(reader);
    let stdout = async_compat::Compat::new(writer2);

    let result = run_bridge(
        mock,
        MockJetStreamClient::new(),
        &config,
        stdout,
        stdin,
        std::future::ready(()),
    )
    .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn run_bridge_exits_on_io_close() {
    let mock = AdvancedMockNatsClient::new();
    let _sub = mock.inject_messages();
    let config = acp_nats::Config::new(
        acp_nats::AcpPrefix::new("acp").unwrap(),
        acp_nats::NatsConfig::new(vec!["localhost:4222".to_string()], trogon_nats::NatsAuth::None),
    );

    let (reader, writer) = tokio::io::duplex(1024);
    let (_reader2, writer2) = tokio::io::duplex(1024);
    drop(writer);
    let stdin = async_compat::Compat::new(reader);
    let stdout = async_compat::Compat::new(writer2);

    let result = run_bridge(
        mock,
        MockJetStreamClient::new(),
        &config,
        stdout,
        stdin,
        std::future::pending(),
    )
    .await;

    assert!(result.is_ok());
}
