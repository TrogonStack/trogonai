use super::*;
use crate::credential_value::{CredentialValue, NKeySeed};
use trogon_std::env::InMemoryEnv;

fn literal_text(value: &CredentialValue) -> Option<&str> {
    match value {
        CredentialValue::Literal(secret) => Some(secret.as_str()),
        _ => None,
    }
}

fn env_with(pairs: &[(&str, &str)]) -> InMemoryEnv {
    let env = InMemoryEnv::new();
    for &(name, value) in pairs {
        env.set(name, value);
    }
    env
}

fn settings(pairs: &[(&str, &str)]) -> NatsSettings {
    NatsSettings::from_env(&env_with(pairs))
}

fn config(pairs: &[(&str, &str)]) -> NatsConfig {
    NatsConfig::from_env(&env_with(pairs))
}

fn tls(pairs: &[(&str, &str)]) -> Result<NatsTls, TlsMisconfiguration> {
    config(pairs).tls
}

#[test]
fn defaults_to_localhost_without_auth() {
    let config = config(&[]);

    assert_eq!(config.servers, vec!["localhost:4222"]);
    assert!(matches!(config.auth, NatsAuth::None));
}

#[test]
fn parses_comma_separated_servers() {
    let config = config(&[("NATS_URL", "host1:4222 , host2:4222 , host3:4222")]);

    assert_eq!(config.servers, vec!["host1:4222", "host2:4222", "host3:4222"]);
}

#[test]
fn empty_url_falls_back_to_default() {
    assert_eq!(config(&[("NATS_URL", "")]).servers, vec!["localhost:4222"]);
}

fn token_text(config: &NatsConfig) -> Option<&str> {
    match &config.auth {
        NatsAuth::Token(token) => literal_text(token),
        _ => None,
    }
}

#[test]
fn creds_win_over_everything() {
    let config = config(&[
        ("NATS_CREDS", "/c.creds"),
        ("NATS_NKEY", "/n.seed"),
        ("NATS_USER", "alice"),
        ("NATS_PASSWORD", "pw"),
        ("NATS_TOKEN", "tok"),
        ("NATS_JWT", "jwt"),
    ]);

    assert!(
        matches!(config.auth, NatsAuth::Credentials(Location::Path(ref p)) if p == std::path::Path::new("/c.creds"))
    );
}

#[test]
fn creds_accept_file_scheme() {
    let config = config(&[("NATS_CREDS", "file:///c.creds")]);

    assert!(
        matches!(config.auth, NatsAuth::Credentials(Location::Path(ref p)) if p == std::path::Path::new("/c.creds"))
    );
}

#[test]
fn nkey_wins_over_user_password_token_and_jwt() {
    let config = config(&[
        ("NATS_NKEY", "/n.seed"),
        ("NATS_USER", "alice"),
        ("NATS_PASSWORD", "pw"),
        ("NATS_TOKEN", "tok"),
        ("NATS_JWT", "jwt"),
    ]);

    assert!(matches!(
        config.auth,
        NatsAuth::NKey(NKeySeed::File(Location::Path(ref p))) if p == std::path::Path::new("/n.seed")
    ));
}

#[test]
fn nkey_accepts_an_inline_seed() {
    let seed = nkeys::KeyPair::new_user().seed().unwrap();
    let config = config(&[("NATS_NKEY", &seed)]);

    assert!(matches!(config.auth, NatsAuth::NKey(NKeySeed::Inline(ref s)) if s.as_str() == seed));
}

#[test]
fn nkey_accepts_a_seed_file_location() {
    let config = config(&[("NATS_NKEY", "file:///n.seed")]);

    assert!(matches!(
        config.auth,
        NatsAuth::NKey(NKeySeed::File(Location::Path(ref p))) if p == std::path::Path::new("/n.seed")
    ));
}

#[test]
fn nkey_accepts_an_env_reference() {
    let config = config(&[("NATS_NKEY", "env://NKEY_SEED")]);

    assert!(matches!(config.auth, NatsAuth::NKey(NKeySeed::Env(ref name)) if name == "NKEY_SEED"));
}

#[test]
fn user_password_wins_over_token_and_jwt() {
    let config = config(&[
        ("NATS_USER", "alice"),
        ("NATS_PASSWORD", "pw"),
        ("NATS_TOKEN", "tok"),
        ("NATS_JWT", "jwt"),
    ]);

    assert!(matches!(
        config.auth,
        NatsAuth::UserPassword { ref user, ref password } if user == "alice" && literal_text(password) == Some("pw")
    ));
}

#[test]
fn token_wins_over_jwt() {
    let config = config(&[("NATS_TOKEN", "tok"), ("NATS_JWT", "jwt"), ("NATS_SEED", "seed")]);

    assert_eq!(token_text(&config), Some("tok"));
}

#[test]
fn jwt_with_seed_signs() {
    let config = config(&[("NATS_JWT", "the-jwt"), ("NATS_SEED", "the-seed")]);

    assert!(matches!(
        config.auth,
        NatsAuth::Jwt { ref jwt, seed: Some(ref seed) }
            if literal_text(jwt) == Some("the-jwt") && literal_text(seed) == Some("the-seed")
    ));
}

#[test]
fn jwt_alone_has_no_seed() {
    let config = config(&[("NATS_JWT", "the-jwt")]);

    assert!(matches!(config.auth, NatsAuth::Jwt { seed: None, .. }));
}

#[test]
fn seed_alone_is_ignored() {
    assert!(matches!(config(&[("NATS_SEED", "the-seed")]).auth, NatsAuth::None));
}

#[test]
fn password_alone_is_ignored() {
    assert!(matches!(config(&[("NATS_PASSWORD", "pw")]).auth, NatsAuth::None));
}

#[test]
fn user_without_password_becomes_token() {
    assert_eq!(token_text(&config(&[("NATS_USER", "tok")])), Some("tok"));
}

#[test]
fn token_variable_wins_over_user_without_password() {
    let config = config(&[("NATS_USER", "from-user"), ("NATS_TOKEN", "from-token")]);

    assert_eq!(token_text(&config), Some("from-token"));
}

#[test]
fn empty_password_makes_user_a_token() {
    let config = config(&[("NATS_USER", "tok"), ("NATS_PASSWORD", "")]);

    assert_eq!(token_text(&config), Some("tok"));
}

#[test]
fn token_variable_applies_alone() {
    assert_eq!(token_text(&config(&[("NATS_TOKEN", "my-token")])), Some("my-token"));
}

#[test]
fn secret_values_may_reference_files_and_variables() {
    let config = config(&[
        ("NATS_USER", "u"),
        ("NATS_PASSWORD", "env://PW"),
        ("NATS_TOKEN", "file:///run/token"),
    ]);

    assert!(matches!(
        config.auth,
        NatsAuth::UserPassword { password: CredentialValue::Env(ref name), .. } if name == "PW"
    ));
    assert!(matches!(
        NatsConfig::from_env(&env_with(&[("NATS_TOKEN", "file:///run/token")])).auth,
        NatsAuth::Token(CredentialValue::File(_))
    ));
}

#[test]
fn unsupported_value_scheme_is_kept_for_connect_time() {
    let config = config(&[("NATS_TOKEN", "op://vault/item/field")]);

    assert!(matches!(config.auth, NatsAuth::Token(CredentialValue::Unsupported(ref scheme)) if scheme == "op"));
}

#[test]
fn tls_is_on_by_default() {
    assert_eq!(tls(&[]), Ok(NatsTls::default()));
}

#[test]
fn tls_needs_cert_and_key_together() {
    assert_eq!(
        tls(&[("NATS_CERT", "/c.pem")]),
        Err(TlsMisconfiguration::IncompleteClientCertificate(
            IncompleteClientCertificate::MissingKey
        ))
    );
    assert_eq!(
        tls(&[("NATS_KEY", "/k.pem")]),
        Err(TlsMisconfiguration::IncompleteClientCertificate(
            IncompleteClientCertificate::MissingCert
        ))
    );
}

#[test]
fn tls_reads_the_client_certificate() {
    assert_eq!(
        tls(&[("NATS_CERT", "/c.pem"), ("NATS_KEY", "file:///k.pem")]),
        Ok(NatsTls::Enabled {
            ca: None,
            client_cert: Some(ClientCertificate {
                cert: Location::Path("/c.pem".into()),
                key: Location::Path("/k.pem".into()),
            }),
        })
    );
}

#[test]
fn tls_ca_is_read() {
    assert_eq!(
        tls(&[("NATS_CA", "/ca.pem")]),
        Ok(NatsTls::Enabled {
            ca: Some(Location::Path("/ca.pem".into())),
            client_cert: None,
        })
    );
}

#[test]
fn tls_disabled_accepts_true_one_and_yes_in_any_case() {
    for value in ["true", "TRUE", "True", "1", "yes", "YES", "Yes"] {
        assert_eq!(tls(&[("NATS_TLS_DISABLED", value)]), Ok(NatsTls::Disabled), "{value}");
    }
}

#[test]
fn empty_tls_disabled_leaves_tls_on() {
    assert_eq!(tls(&[("NATS_TLS_DISABLED", "")]), Ok(NatsTls::default()));
}

#[test]
fn tls_disabled_rejects_every_other_value() {
    for value in ["false", "0", "no", "off", "on", "enabled", " true"] {
        assert_eq!(
            tls(&[("NATS_TLS_DISABLED", value)]),
            Err(TlsMisconfiguration::UnrecognizedDisabledValue(value.to_owned())),
            "{value}"
        );
    }
}

#[test]
fn tls_disabled_conflicts_with_every_tls_setting() {
    for setting in [ENV_NATS_CA, ENV_NATS_CERT, ENV_NATS_KEY] {
        assert_eq!(
            tls(&[("NATS_TLS_DISABLED", "true"), (setting, "/x.pem")]),
            Err(TlsMisconfiguration::DisabledAlongside(setting)),
            "{setting}"
        );
    }
}

#[test]
fn resolve_reads_tls_disabled_only_from_the_environment_it_is_given() {
    let settings = NatsSettings {
        url: Some("nats://x:4222".into()),
        ..NatsSettings::default()
    };

    assert_eq!(settings.resolve(&InMemoryEnv::new()).tls, Ok(NatsTls::default()));
    assert_eq!(
        settings.resolve(&env_with(&[("NATS_TLS_DISABLED", "yes")])).tls,
        Ok(NatsTls::Disabled)
    );
}

#[test]
fn settings_debug_redacts_secrets() {
    let debug = format!(
        "{:?}",
        settings(&[
            ("NATS_PASSWORD", "super-secret-password"),
            ("NATS_TOKEN", "super-secret-token"),
            ("NATS_JWT", "super-secret-jwt"),
            ("NATS_SEED", "super-secret-seed"),
            ("NATS_NKEY", "super-secret-nkey"),
        ])
    );

    for secret in ["password", "token", "jwt", "seed", "nkey"] {
        assert!(!debug.contains(&format!("super-secret-{secret}")));
    }
    assert!(debug.contains("SecretString(***)"));
}
