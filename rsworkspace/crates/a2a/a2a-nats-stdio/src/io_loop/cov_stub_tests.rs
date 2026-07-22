use a2a_nats::client::A2aClient;
use a2a_nats::{A2aAgentId, A2aPrefix};
use trogon_nats::AdvancedMockNatsClient;
use trogon_nats::jetstream::mocks::MockJetStreamConsumerFactory;

#[tokio::test]
async fn run_io_loop_stub_is_callable() {
    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamConsumerFactory::new();
    let client = A2aClient::new(
        A2aPrefix::new("a2a").unwrap(),
        A2aAgentId::new("bot").unwrap(),
        nats,
        js,
    );
    let (stdin_reader, _stdin_writer) = tokio::io::duplex(64);
    let (_stdout_reader, stdout_writer) = tokio::io::duplex(64);
    super::run_io_loop(client, stdin_reader, stdout_writer, std::future::ready(()))
        .await
        .unwrap();
}
