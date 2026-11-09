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
}

#[test]
fn from_url_convenience() {
    let config = NatsConfig::from_url("nats://custom:4222");

    assert_eq!(config.servers, vec!["nats://custom:4222"]);
    assert!(matches!(config.auth, NatsAuth::None));
    assert_eq!(config.tls, Ok(NatsTls::default()));
}

#[test]
fn with_tls_attaches_tls() {
    let tls = NatsTls::Enabled {
        ca: Location::parse("/ca.pem"),
        client_cert: None,
    };

    assert_eq!(NatsConfig::from_url("nats://x:4222").with_tls(tls.clone()).tls, Ok(tls));
    assert_eq!(
        NatsConfig::from_url("nats://x:4222").with_tls(NatsTls::Disabled).tls,
        Ok(NatsTls::Disabled)
    );
}

#[test]
fn description_matches_variant() {
    assert_eq!(
        NatsAuth::Credentials(Location::Path("/a".into())).description(),
        "credentials file"
    );
    assert_eq!(
        NatsAuth::NKey(NKeySeed::File(Location::Path("/k".into()))).description(),
        "NKey"
    );
    assert_eq!(NatsAuth::Token(value("t")).description(), "token");
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

    let token = format!("{:?}", NatsAuth::Token(value("super-secret-token")));
    assert!(!token.contains("super-secret-token"));

    let seed = nkeys::KeyPair::new_user().seed().unwrap();
    let nkey = format!("{:?}", NatsAuth::NKey(NKeySeed::parse(&seed).unwrap()));
    assert!(!nkey.contains(&seed));
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
