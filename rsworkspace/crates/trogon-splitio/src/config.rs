use trogon_std::env::ReadEnv;

const DEFAULT_EVALUATOR_URL: &str = "http://localhost:7548";

/// Configuration for the Split Evaluator HTTP client.
///
/// Environment variables:
/// - `SPLIT_EVALUATOR_URL`        — base URL of the Split Evaluator service (default: `http://localhost:7548`)
/// - `SPLIT_EVALUATOR_AUTH_TOKEN` — authentication token expected by the evaluator (`Authorization` header)
#[derive(Debug, Clone)]
pub struct SplitConfig {
    /// Base URL of the running `splitsoftware/split-evaluator` instance.
    pub evaluator_url: String,
    /// Token used in `Authorization: <token>` when calling the evaluator.
    /// Must match `SPLIT_EVALUATOR_AUTH_TOKEN` configured on the evaluator.
    pub auth_token: String,
}

impl SplitConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        Self {
            evaluator_url: env
                .var("SPLIT_EVALUATOR_URL")
                .unwrap_or_else(|_| DEFAULT_EVALUATOR_URL.to_string()),
            auth_token: env.var("SPLIT_EVALUATOR_AUTH_TOKEN").unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_std::env::InMemoryEnv;

    #[test]
    fn defaults_when_no_env_vars() {
        let env = InMemoryEnv::new();
        let cfg = SplitConfig::from_env(&env);
        assert_eq!(cfg.evaluator_url, DEFAULT_EVALUATOR_URL);
        assert!(cfg.auth_token.is_empty());
    }

    #[test]
    fn reads_all_env_vars() {
        let env = InMemoryEnv::new();
        env.set("SPLIT_EVALUATOR_URL", "http://evaluator:9090");
        env.set("SPLIT_EVALUATOR_AUTH_TOKEN", "my-secret-token");
        let cfg = SplitConfig::from_env(&env);
        assert_eq!(cfg.evaluator_url, "http://evaluator:9090");
        assert_eq!(cfg.auth_token, "my-secret-token");
    }

    #[test]
    fn custom_url_with_default_token() {
        let env = InMemoryEnv::new();
        env.set("SPLIT_EVALUATOR_URL", "http://split:7548");
        let cfg = SplitConfig::from_env(&env);
        assert_eq!(cfg.evaluator_url, "http://split:7548");
        assert!(cfg.auth_token.is_empty());
    }

    /// Trailing slashes in the URL are preserved in SplitConfig — stripping
    /// is the responsibility of SplitClient::new.  This test documents that
    /// contract so future readers understand where trimming happens.
    #[test]
    fn url_with_trailing_slashes_is_stored_as_is() {
        let env = InMemoryEnv::new();
        env.set("SPLIT_EVALUATOR_URL", "http://localhost:7548///");
        let cfg = SplitConfig::from_env(&env);
        assert_eq!(cfg.evaluator_url, "http://localhost:7548///");
    }

    /// An empty URL string is stored as-is — the client will fail at request
    /// time rather than at config-construction time.
    #[test]
    fn empty_url_is_stored_as_empty_string() {
        let env = InMemoryEnv::new();
        env.set("SPLIT_EVALUATOR_URL", "");
        let cfg = SplitConfig::from_env(&env);
        assert_eq!(cfg.evaluator_url, "");
    }
}
