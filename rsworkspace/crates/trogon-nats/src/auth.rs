use std::path::PathBuf;
use trogon_std::env::ReadEnv;

use crate::constants::{
    DEFAULT_NATS_URL, ENV_NATS_CREDS, ENV_NATS_NKEY, ENV_NATS_PASSWORD, ENV_NATS_TOKEN,
    ENV_NATS_URL, ENV_NATS_USER,
};

/// NATS authentication method.
///
/// When resolved from environment variables, priority order is:
/// 1. Credentials file (`NATS_CREDS`)
/// 2. NKey (`NATS_NKEY`)
/// 3. User/Password (`NATS_USER` + `NATS_PASSWORD`)
/// 4. Token (`NATS_TOKEN`)
/// 5. No auth
#[derive(Debug, Clone)]
pub enum NatsAuth {
    Credentials(PathBuf),
    NKey(String),
    UserPassword { user: String, password: String },
    Token(String),
    None,
}

impl NatsAuth {
    pub fn description(&self) -> &'static str {
        match self {
            Self::Credentials(_) => "credentials file",
            Self::NKey(_) => "NKey",
            Self::UserPassword { .. } => "user/password",
            Self::Token(_) => "token",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone)]
pub struct NatsConfig {
    pub servers: Vec<String>,
    pub auth: NatsAuth,
}

impl NatsConfig {
    pub fn new(servers: Vec<String>, auth: NatsAuth) -> Self {
        Self { servers, auth }
    }

    pub fn from_url(url: impl Into<String>) -> Self {
        Self {
            servers: vec![url.into()],
            auth: NatsAuth::None,
        }
    }

    /// Build config from environment variables.
    ///
    /// - `NATS_URL`: comma-separated server list (default: `localhost:4222`)
    /// - Auth resolved via `NATS_CREDS` > `NATS_NKEY` > `NATS_USER`+`NATS_PASSWORD` > `NATS_TOKEN` > none
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        Self {
            servers: servers_from_env(env),
            auth: auth_from_env(env),
        }
    }
}

fn servers_from_env<E: ReadEnv>(env: &E) -> Vec<String> {
    let raw = env
        .var(ENV_NATS_URL)
        .unwrap_or_else(|_| DEFAULT_NATS_URL.to_string());
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn auth_from_env<E: ReadEnv>(env: &E) -> NatsAuth {
    if let Ok(creds_path) = env.var(ENV_NATS_CREDS) {
        return NatsAuth::Credentials(PathBuf::from(creds_path));
    }
    if let Ok(nkey) = env.var(ENV_NATS_NKEY) {
        return NatsAuth::NKey(nkey);
    }
    if let (Ok(user), Ok(password)) = (env.var(ENV_NATS_USER), env.var(ENV_NATS_PASSWORD)) {
        if !user.is_empty() && !password.is_empty() {
            return NatsAuth::UserPassword { user, password };
        }
    }
    if let Ok(token) = env.var(ENV_NATS_TOKEN) {
        return NatsAuth::Token(token);
    }
    NatsAuth::None
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_std::env::InMemoryEnv;

    #[test]
    fn from_env_defaults_to_localhost_with_no_auth() {
        let env = InMemoryEnv::new();
        let config = NatsConfig::from_env(&env);

        assert_eq!(config.servers, vec!["localhost:4222"]);
        assert!(matches!(config.auth, NatsAuth::None));
    }

    #[test]
    fn from_env_parses_comma_separated_servers() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "host1:4222 , host2:4222 , host3:4222");

        let config = NatsConfig::from_env(&env);

        assert_eq!(
            config.servers,
            vec!["host1:4222", "host2:4222", "host3:4222"]
        );
    }

    #[test]
    fn from_env_credentials_take_priority() {
        let env = InMemoryEnv::new();
        env.set("NATS_CREDS", "/path/to/creds");
        env.set("NATS_NKEY", "some-nkey");
        env.set("NATS_TOKEN", "some-token");

        let config = NatsConfig::from_env(&env);

        assert!(
            matches!(config.auth, NatsAuth::Credentials(p) if p == std::path::Path::new("/path/to/creds"))
        );
    }

    #[test]
    fn from_env_nkey_over_user_password_and_token() {
        let env = InMemoryEnv::new();
        env.set("NATS_NKEY", "my-nkey");
        env.set("NATS_USER", "user");
        env.set("NATS_PASSWORD", "pass");
        env.set("NATS_TOKEN", "tok");

        assert!(matches!(NatsConfig::from_env(&env).auth, NatsAuth::NKey(k) if k == "my-nkey"));
    }

    #[test]
    fn from_env_user_password_over_token() {
        let env = InMemoryEnv::new();
        env.set("NATS_USER", "user");
        env.set("NATS_PASSWORD", "pass");
        env.set("NATS_TOKEN", "tok");

        assert!(matches!(
            NatsConfig::from_env(&env).auth,
            NatsAuth::UserPassword { user, password } if user == "user" && password == "pass"
        ));
    }

    #[test]
    fn from_env_token_when_nothing_else() {
        let env = InMemoryEnv::new();
        env.set("NATS_TOKEN", "my-token");

        assert!(matches!(NatsConfig::from_env(&env).auth, NatsAuth::Token(t) if t == "my-token"));
    }

    #[test]
    fn from_env_requires_both_user_and_password() {
        let env = InMemoryEnv::new();
        env.set("NATS_USER", "user");
        // no NATS_PASSWORD

        assert!(matches!(NatsConfig::from_env(&env).auth, NatsAuth::None));
    }

    #[test]
    fn from_env_requires_both_user_and_password_only_password_set() {
        let env = InMemoryEnv::new();
        env.set("NATS_PASSWORD", "pass");

        assert!(matches!(NatsConfig::from_env(&env).auth, NatsAuth::None));
    }

    #[test]
    fn from_url_convenience() {
        let config = NatsConfig::from_url("nats://custom:4222");

        assert_eq!(config.servers, vec!["nats://custom:4222"]);
        assert!(matches!(config.auth, NatsAuth::None));
    }

    // ── servers_from_env edge cases ───────────────────────────────────────────

    /// NATS_URL set to whitespace-only entries (e.g. "  ,  ,  ") produces an
    /// EMPTY server list — NOT the default `localhost:4222`.
    /// The default is only used when the env var is absent.
    #[test]
    fn servers_from_env_whitespace_only_nats_url_produces_empty_vec() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "  ,  ,  ");
        let config = NatsConfig::from_env(&env);
        assert!(
            config.servers.is_empty(),
            "whitespace-only NATS_URL must produce an empty server list, got: {:?}",
            config.servers
        );
    }

    /// A trailing comma in NATS_URL is silently ignored — the empty segment
    /// is filtered out and only the valid host remains.
    #[test]
    fn servers_from_env_trailing_comma_ignored() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "host1:4222,");
        let config = NatsConfig::from_env(&env);
        assert_eq!(config.servers, vec!["host1:4222"]);
    }

    /// A single-entry NATS_URL with surrounding whitespace is trimmed correctly.
    #[test]
    fn servers_from_env_single_entry_with_spaces() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "  nats://host:4222  ");
        let config = NatsConfig::from_env(&env);
        assert_eq!(config.servers, vec!["nats://host:4222"]);
    }

    /// A whitespace-only segment in the middle of a comma-separated NATS_URL
    /// is trimmed to an empty string and filtered out, so only the valid
    /// hosts on either side survive.
    #[test]
    fn servers_from_env_middle_whitespace_only_segment_is_filtered() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "host1:4222, ,host2:4222");
        let config = NatsConfig::from_env(&env);
        assert_eq!(config.servers, vec!["host1:4222", "host2:4222"]);
    }

    /// A double comma in NATS_URL (e.g. `"host1,,host2"`) produces an empty
    /// segment after splitting on `,`; the trim+filter chain must discard it
    /// so only the two valid hosts survive.
    #[test]
    fn servers_from_env_double_comma_filters_empty_segment() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "host1:4222,,host2:4222");
        let config = NatsConfig::from_env(&env);
        assert_eq!(config.servers, vec!["host1:4222", "host2:4222"]);
    }

    /// `NATS_PASSWORD=""` (present but empty) must fall through to `None` —
    /// an empty password is treated the same as an absent one so that
    /// misconfigured deployments fail at connection time with a clear
    /// "no auth configured" message rather than a confusing NATS auth error.
    #[test]
    fn auth_from_env_empty_password_falls_through_to_none() {
        let env = InMemoryEnv::new();
        env.set("NATS_USER", "alice");
        env.set("NATS_PASSWORD", "");

        let auth = NatsConfig::from_env(&env).auth;
        assert!(
            matches!(auth, NatsAuth::None),
            "empty NATS_PASSWORD must fall through to NatsAuth::None, got: {auth:?}"
        );
    }

    /// `NATS_USER=""` (present but empty) must also fall through — an empty
    /// user is equally a misconfiguration.
    #[test]
    fn auth_from_env_empty_user_falls_through_to_none() {
        let env = InMemoryEnv::new();
        env.set("NATS_USER", "");
        env.set("NATS_PASSWORD", "secret");

        let auth = NatsConfig::from_env(&env).auth;
        assert!(
            matches!(auth, NatsAuth::None),
            "empty NATS_USER must fall through to NatsAuth::None, got: {auth:?}"
        );
    }

    #[test]
    fn description_matches_variant() {
        assert_eq!(
            NatsAuth::Credentials("/a".into()).description(),
            "credentials file"
        );
        assert_eq!(NatsAuth::NKey("k".into()).description(), "NKey");
        assert_eq!(
            NatsAuth::UserPassword {
                user: "u".into(),
                password: "p".into()
            }
            .description(),
            "user/password"
        );
        assert_eq!(NatsAuth::Token("t".into()).description(), "token");
        assert_eq!(NatsAuth::None.description(), "none");
    }
}
