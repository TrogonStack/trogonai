// Violation fixture for scripts/ci/check-direct-nats-publishes.sh — must stay whitelisted or live under tests/.
#![allow(dead_code)]

async fn forbidden_direct_publish(client: async_nats::Client) {
    let _ = client
        .publish("mcp.gateway.request.github.tools.call".to_string(), "payload".into())
        .await;
}
