use super::*;
use std::path::PathBuf;
use trogon_std::env::InMemoryEnv;

fn value(raw: &str) -> CredentialValue {
    CredentialValue::parse(raw).unwrap()
}

#[test]
fn from_env_delegates_to_settings() {
    let env = InMemoryEnv::new();
    env.set("NATS_URL", "a:4222,b:4222");
    env.set("NATS_CREDS", "/path/to/creds");
    env.set("NATS_TOKEN", "tok");

    let config = NatsConfig::from_env(&env);

    assert_eq!(config.servers, vec!["a:4222", "b:4222"]);
    assert!(matches!(config.auth, NatsAuth::Credentials(_)));
    assert!(config.token.is_some());
}

#[test]
fn from_url_convenience() {
    let config = NatsConfig::from_url("nats://custom:4222");

    assert_eq!(config.servers, vec!["nats://custom:4222"]);
    assert!(matches!(config.auth, NatsAuth::None));
    assert!(config.token.is_none());
    assert!(!config.tls.is_configured());
}

#[test]
fn builders_attach_token_and_tls() {
    let tls = NatsTls::new(None, None, Location::parse("/ca.pem"));
    let config = NatsConfig::from_url("nats://x:4222")
        .with_token(value("tok"))
        .with_tls(tls.clone());

    assert!(config.token.is_some());
    assert_eq!(config.tls, tls);
}

#[test]
fn description_matches_variant() {
    assert_eq!(
        NatsAuth::Credentials(Location::Path("/a".into())).description(),
        "credentials file"
    );
    assert_eq!(NatsAuth::NKey(Location::Path("/k".into())).description(), "NKey");
    assert_eq!(
        NatsAuth::UserPassword {
            user: "u".into(),
            password: value("p")
        }
        .description(),
        "user/password"
    );
    assert_eq!(
        NatsAuth::Jwt {
            jwt: value("j"),
            seed: Some(value("s"))
        }
        .description(),
        "JWT"
    );
    assert_eq!(
        NatsAuth::Jwt {
            jwt: value("j"),
            seed: None
        }
        .description(),
        "JWT without signature"
    );
    assert_eq!(NatsAuth::None.description(), "none");
}

#[test]
fn debug_redacts_secrets() {
    let user_password = format!(
        "{:?}",
        NatsAuth::UserPassword {
            user: "alice".into(),
            password: value("super-secret-password"),
        }
    );
    assert!(user_password.contains("alice"));
    assert!(!user_password.contains("super-secret-password"));
    assert!(user_password.contains("<redacted>"));

    let jwt = format!(
        "{:?}",
        NatsAuth::Jwt {
            jwt: value("super-secret-jwt"),
            seed: Some(value("super-secret-seed")),
        }
    );
    assert!(!jwt.contains("super-secret-jwt"));
    assert!(!jwt.contains("super-secret-seed"));

    let config = format!(
        "{:?}",
        NatsConfig::from_url("x:1").with_token(value("super-secret-token"))
    );
    assert!(!config.contains("super-secret-token"));
}

#[test]
fn debug_shows_non_secret_variants() {
    let creds = format!(
        "{:?}",
        NatsAuth::Credentials(Location::Path(PathBuf::from("/etc/nats/app.creds")))
    );
    assert!(creds.contains("/etc/nats/app.creds"));
    assert_eq!(format!("{:?}", NatsAuth::None), "None");
}
