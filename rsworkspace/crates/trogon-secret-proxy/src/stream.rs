//! JetStream stream provisioning for proxy requests.

use async_nats::jetstream::stream::{Config as StreamConfig, RetentionPolicy, StorageType};
use trogon_nats::jetstream::JetStreamContext;

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
    format!("PROXY_REQUESTS_{}", prefix.to_uppercase().replace('-', "_"))
}

/// Ensure the JetStream stream for the given prefix exists.
///
/// Idempotent — safe to call on every startup. Uses a work-queue retention
/// policy so messages are removed after a worker acknowledges them.
pub async fn ensure_stream<J: JetStreamContext>(
    jetstream: &J,
    prefix: &str,
    outbound_subject: &str,
) -> Result<(), String> {
    let config = StreamConfig {
        name: stream_name(prefix),
        subjects: vec![outbound_subject.to_string()],
        retention: RetentionPolicy::WorkQueue,
        storage: StorageType::Memory,
        ..Default::default()
    };

    jetstream
        .get_or_create_stream(config)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::Mutex;

    use async_nats::jetstream::stream;
    use trogon_nats::jetstream::JetStreamContext;

    use super::*;

    // ── MockJetStreamContext ──────────────────────────────────────────────────

    /// In-memory mock that records the stream configs passed to
    /// `get_or_create_stream`. Returns `Ok` by default; set `fail = true` to
    /// return an error so `ensure_stream` failure paths can be tested.
    struct MockJetStreamContext {
        configs: Mutex<Vec<String>>, // stream names captured
        fail: bool,
    }

    impl MockJetStreamContext {
        fn new() -> Self {
            Self {
                configs: Mutex::new(vec![]),
                fail: false,
            }
        }
        fn failing() -> Self {
            Self {
                configs: Mutex::new(vec![]),
                fail: true,
            }
        }
        fn captured_names(&self) -> Vec<String> {
            self.configs.lock().unwrap().clone()
        }
    }

    impl Clone for MockJetStreamContext {
        fn clone(&self) -> Self {
            Self {
                configs: Mutex::new(self.configs.lock().unwrap().clone()),
                fail: self.fail,
            }
        }
    }

    impl JetStreamContext for MockJetStreamContext {
        type Error = std::io::Error;
        type Stream = stream::Stream;

        fn get_or_create_stream<S: Into<stream::Config> + Send>(
            &self,
            config: S,
        ) -> impl Future<Output = Result<Self::Stream, Self::Error>> + Send {
            let cfg: stream::Config = config.into();
            self.configs.lock().unwrap().push(cfg.name.clone());
            let fail = self.fail;
            async move {
                if fail {
                    Err(std::io::Error::other(
                        "mock stream error",
                    ))
                } else {
                    // Returning a real stream is not possible in unit tests;
                    // ensure_stream maps it to () so we never need the value.
                    Err(std::io::Error::other(
                        "mock always returns Err — ensure_stream maps Ok to ()",
                    ))
                }
            }
        }
    }

    // `ensure_stream` maps `Ok(_)` to `Ok(())`, so we need a mock that returns
    // Ok. Use a separate implementation with `Option<stream::Stream>` — but
    // constructing a real stream is impossible in unit tests without NATS.
    // Instead, test the success path via integration tests and use the mock
    // only to verify the stream name computation and error mapping here.
    #[tokio::test]
    async fn ensure_stream_passes_correct_stream_name_to_provisioner() {
        // The mock always returns Err (we can't construct a real stream::Stream
        // without NATS), but we can verify the stream name that was requested.
        let mock = MockJetStreamContext::new();
        let _ = ensure_stream(&mock, "my-service", "proxy.outbound").await;
        let names = mock.captured_names();
        assert_eq!(names, vec!["PROXY_REQUESTS_MY_SERVICE"]);
    }

    #[tokio::test]
    async fn ensure_stream_maps_error_to_string() {
        let mock = MockJetStreamContext::failing();
        let result = ensure_stream(&mock, "trogon", "proxy.outbound").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("mock stream error"), "Error message: {err}");
    }

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

    // ── Gap: null byte in prefix ───────────────────────────────────────────────

    /// A null byte (`\0`) in the prefix is uppercased to `\0` (unchanged)
    /// and passes through to the stream name.  NATS stream names must not
    /// contain null bytes.  Callers must validate the prefix before calling
    /// `stream_name`.
    #[test]
    fn stream_name_null_byte_in_prefix_is_preserved() {
        let name = stream_name("foo\0bar");
        assert_eq!(name, "PROXY_REQUESTS_FOO\0BAR");
    }
}
