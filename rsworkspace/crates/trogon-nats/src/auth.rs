use std::path::PathBuf;
use trogon_std::env::ReadEnv;

const ENV_NATS_URL: &str = "NATS_URL";
const ENV_NATS_CREDS: &str = "NATS_CREDS";
const ENV_NATS_NKEY: &str = "NATS_NKEY";
const ENV_NATS_USER: &str = "NATS_USER";
const ENV_NATS_PASSWORD: &str = "NATS_PASSWORD";
const ENV_NATS_TOKEN: &str = "NATS_TOKEN";

const DEFAULT_NATS_URL: &str = "localhost:4222";

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
        return NatsAuth::UserPassword { user, password };
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

    /// Only `NATS_PASSWORD` set (no `NATS_USER`) must also fall through to
    /// `NatsAuth::None` — both halves of the tuple pattern must succeed.
    #[test]
    fn from_env_requires_both_user_and_password_only_password_set() {
        let env = InMemoryEnv::new();
        env.set("NATS_PASSWORD", "pass");
        // no NATS_USER

        assert!(matches!(NatsConfig::from_env(&env).auth, NatsAuth::None));
    }

    #[test]
    fn from_url_convenience() {
        let config = NatsConfig::from_url("nats://custom:4222");

        assert_eq!(config.servers, vec!["nats://custom:4222"]);
        assert!(matches!(config.auth, NatsAuth::None));
    }

    /// `NatsConfig::new()` stores the given servers and auth as-is.
    #[test]
    fn new_stores_servers_and_auth() {
        let servers = vec!["host1:4222".to_string(), "host2:4222".to_string()];
        let config = NatsConfig::new(servers.clone(), NatsAuth::Token("tok".into()));

        assert_eq!(config.servers, servers);
        assert!(matches!(config.auth, NatsAuth::Token(t) if t == "tok"));
    }

    /// `NatsConfig::new()` with empty server list preserves the empty list
    /// (callers are responsible for validation before connecting).
    #[test]
    fn new_with_empty_servers() {
        let config = NatsConfig::new(vec![], NatsAuth::None);
        assert!(config.servers.is_empty());
    }

    /// `from_url("")` wraps the empty string directly in the server list —
    /// it does NOT apply the trim+filter that `from_env` does.
    /// This asymmetry is intentional: `from_url` is a direct constructor.
    #[test]
    fn from_url_empty_string_preserves_empty_entry() {
        let config = NatsConfig::from_url("");

        assert_eq!(
            config.servers,
            vec!["".to_string()],
            "from_url stores the value verbatim, unlike from_env which filters empty segments"
        );
    }

    /// A `NATS_URL` consisting only of commas and whitespace produces an empty
    /// server list after the trim+filter step.  Callers are responsible for
    /// detecting this before attempting a connection.
    #[test]
    fn from_env_nats_url_all_whitespace_and_commas_produces_empty_servers() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "  ,  ,  ");

        let config = NatsConfig::from_env(&env);

        assert!(
            config.servers.is_empty(),
            "all-whitespace NATS_URL should produce an empty server list; got {:?}",
            config.servers
        );
    }

    /// An explicit empty string for `NATS_URL` also results in an empty list.
    #[test]
    fn from_env_nats_url_empty_string_produces_empty_servers() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "");

        let config = NatsConfig::from_env(&env);

        assert!(
            config.servers.is_empty(),
            "empty NATS_URL should produce an empty server list; got {:?}",
            config.servers
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
