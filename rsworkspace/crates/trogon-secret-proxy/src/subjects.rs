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

/// Core NATS subject for vault store requests (admin).
pub fn vault_store(prefix: &str) -> String {
    format!("{}.vault.store", prefix)
}

/// Core NATS subject for vault rotate requests (admin).
pub fn vault_rotate(prefix: &str) -> String {
    format!("{}.vault.rotate", prefix)
}

/// Core NATS subject for vault revoke requests (admin).
pub fn vault_revoke(prefix: &str) -> String {
    format!("{}.vault.revoke", prefix)
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

    // ── Gap: NATS wildcards in prefix ─────────────────────────────────────────

    /// `outbound` and `reply` do not validate the prefix — NATS wildcard
    /// characters (`*`, `>`) pass through unchanged.  The resulting subjects
    /// would act as wildcard subscriptions rather than literal names.
    /// Callers must ensure the prefix contains no NATS special characters.
    #[test]
    fn outbound_prefix_with_nats_wildcard_is_not_sanitized() {
        assert_eq!(outbound("trogon.*"), "trogon.*.proxy.http.outbound");
        assert_eq!(outbound("trogon.>"), "trogon.>.proxy.http.outbound");
    }

    /// A correlation ID containing dots produces extra subject segments.
    /// NATS treats each `.` as a token separator, so `"a.b.c"` as a
    /// correlation ID yields `"trogon.proxy.reply.a.b.c"` — syntactically
    /// valid but may create unexpected subscription matches.
    /// Callers should use UUIDs (hyphens only, no dots) as correlation IDs.
    #[test]
    fn reply_correlation_id_with_dots_produces_extra_segments() {
        assert_eq!(reply("trogon", "a.b.c"), "trogon.proxy.reply.a.b.c");
    }

    // ── Gap 3.1: empty prefix ─────────────────────────────────────────────────

    /// An empty prefix produces subjects with a leading dot.
    /// NATS subjects with leading dots are invalid, so callers must ensure
    /// the prefix is non-empty.  These functions do not validate input.
    #[test]
    fn outbound_empty_prefix_produces_leading_dot() {
        assert_eq!(outbound(""), ".proxy.http.outbound");
    }

    #[test]
    fn reply_empty_prefix_produces_leading_dot() {
        assert_eq!(reply("", "my-correlation-id"), ".proxy.reply.my-correlation-id");
    }

    #[test]
    fn vault_store_subject() {
        assert_eq!(vault_store("trogon"), "trogon.vault.store");
    }

    #[test]
    fn vault_rotate_subject() {
        assert_eq!(vault_rotate("trogon"), "trogon.vault.rotate");
    }

    #[test]
    fn vault_revoke_subject() {
        assert_eq!(vault_revoke("trogon"), "trogon.vault.revoke");
    }
}
