//! JetStream stream provisioning for proxy requests.

use async_nats::jetstream::{self, context::CreateStreamError, stream::Config as StreamConfig};
use async_nats::jetstream::stream::{RetentionPolicy, StorageType};

/// Compute the JetStream stream name for the given prefix.
///
/// Each prefix gets its own stream so that multiple deployments sharing the
/// same NATS cluster (e.g. `production` and `staging`) do not mix messages.
///
/// # Examples
///
/// ```
/// use trogon_secret_proxy::stream::stream_name;
/// assert_eq!(stream_name("trogon"),     "PROXY_REQUESTS_TROGON");
/// assert_eq!(stream_name("my-company"), "PROXY_REQUESTS_MY_COMPANY");
/// ```
pub fn stream_name(prefix: &str) -> String {
    format!(
        "PROXY_REQUESTS_{}",
        prefix.to_uppercase().replace('-', "_")
    )
}

/// Ensure the JetStream stream for the given prefix exists.
///
/// Idempotent — safe to call on every startup. Uses a work-queue retention
/// policy so messages are removed after a worker acknowledges them.
pub async fn ensure_stream(
    jetstream: &jetstream::Context,
    prefix: &str,
    outbound_subject: &str,
) -> Result<jetstream::stream::Stream, CreateStreamError> {
    let config = StreamConfig {
        name: stream_name(prefix),
        subjects: vec![outbound_subject.to_string()],
        retention: RetentionPolicy::WorkQueue,
        storage: StorageType::Memory,
        ..Default::default()
    };

    jetstream.get_or_create_stream(config).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_name_uppercases_prefix() {
        assert_eq!(stream_name("trogon"), "PROXY_REQUESTS_TROGON");
    }

    #[test]
    fn stream_name_replaces_hyphens() {
        assert_eq!(stream_name("my-company"), "PROXY_REQUESTS_MY_COMPANY");
    }

    #[test]
    fn stream_name_different_prefixes_differ() {
        assert_ne!(stream_name("production"), stream_name("staging"));
    }

    // ── Gap: slash not replaced ────────────────────────────────────────────────

    /// `stream_name` only replaces hyphens with underscores.  A slash in the
    /// prefix is NOT replaced and appears verbatim in the stream name.
    ///
    /// NATS stream names must not contain `/`.  Callers are responsible for
    /// validating the prefix before passing it here; `stream_name` does not
    /// perform input validation.
    #[test]
    fn stream_name_slash_is_preserved_not_replaced() {
        // Hyphens are still replaced as usual.
        assert_eq!(stream_name("my-company"), "PROXY_REQUESTS_MY_COMPANY");
        // A slash is NOT replaced — it is uppercased but otherwise preserved.
        assert_eq!(stream_name("trogon/api"), "PROXY_REQUESTS_TROGON/API");
    }

    /// An empty prefix produces `"PROXY_REQUESTS_"` — a trailing underscore
    /// with nothing after it.  Callers must ensure the prefix is non-empty.
    #[test]
    fn stream_name_empty_prefix_produces_trailing_underscore() {
        assert_eq!(stream_name(""), "PROXY_REQUESTS_");
    }
}
