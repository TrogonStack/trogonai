//! NATS subject helpers for the secret proxy.

/// JetStream subject for outbound HTTP requests.
///
/// Published by the proxy, consumed by workers via pull consumer.
pub fn outbound(prefix: &str) -> String {
    format!("{}.proxy.http.outbound", prefix)
}

/// Core NATS reply subject for a specific correlation ID.
///
/// Published by the worker, subscribed to by the proxy.
pub fn reply(prefix: &str, id: &str) -> String {
    format!("{}.proxy.reply.{}", prefix, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbound_subject() {
        assert_eq!(outbound("trogon"), "trogon.proxy.http.outbound");
    }

    #[test]
    fn reply_subject() {
        assert_eq!(
            reply("trogon", "550e8400-e29b-41d4-a716-446655440000"),
            "trogon.proxy.reply.550e8400-e29b-41d4-a716-446655440000"
        );
    }
}
