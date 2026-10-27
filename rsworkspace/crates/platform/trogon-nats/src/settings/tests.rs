use super::*;
use crate::credential_value::CredentialValue;
use trogon_std::env::InMemoryEnv;

fn literal_text(value: &CredentialValue) -> Option<&str> {
    match value {
        CredentialValue::Literal(secret) => Some(secret.as_str()),
        _ => None,
    }
}

fn settings(pairs: &[(&str, &str)]) -> NatsSettings {
    let env = InMemoryEnv::new();
    for &(name, value) in pairs {
        env.set(name, value);
    }
    NatsSettings::from_env(&env)
}

fn config(pairs: &[(&str, &str)]) -> NatsConfig {
    settings(pairs).resolve()
}

#[test]
fn defaults_to_localhost_without_auth() {
    let config = config(&[]);

    assert_eq!(config.servers, vec!["localhost:4222"]);
    assert!(matches!(config.auth, NatsAuth::None));
    assert!(config.token.is_none());
    assert!(!config.tls.is_configured());
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

#[test]
fn user_password_wins_over_everything() {
    let config = config(&[
        ("NATS_USER", "alice"),
        ("NATS_PASSWORD", "pw"),
        ("NATS_CREDS", "/c.creds"),
        ("NATS_NKEY", "/n.seed"),
        ("NATS_JWT", "jwt"),
        ("NATS_SEED", "seed"),
    ]);

    assert!(matches!(
        config.auth,
        NatsAuth::UserPassword { ref user, ref password } if user == "alice" && literal_text(password) == Some("pw")
    ));
    assert!(config.token.is_none());
}

#[test]
fn creds_win_over_nkey_and_jwt() {
    let config = config(&[
        ("NATS_CREDS", "/c.creds"),
        ("NATS_NKEY", "/n.seed"),
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
fn nkey_is_a_seed_file_location_and_beats_jwt() {
    let config = config(&[("NATS_NKEY", "/n.seed"), ("NATS_JWT", "jwt"), ("NATS_SEED", "seed")]);

    assert!(matches!(config.auth, NatsAuth::NKey(Location::Path(ref p)) if p == std::path::Path::new("/n.seed")));
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
    let config = config(&[("NATS_PASSWORD", "pw")]);

    assert!(matches!(config.auth, NatsAuth::None));
    assert!(config.token.is_none());
}

#[test]
fn user_without_password_becomes_token() {
    let config = config(&[("NATS_USER", "tok")]);

    assert!(matches!(config.auth, NatsAuth::None));
    assert!(config.token.as_ref().and_then(literal_text) == Some("tok"));
}

#[test]
fn user_as_token_overrides_token_variable() {
    let config = config(&[("NATS_USER", "from-user"), ("NATS_TOKEN", "from-token")]);

    assert!(config.token.as_ref().and_then(literal_text) == Some("from-user"));
}

#[test]
fn empty_password_makes_user_a_token() {
    let config = config(&[("NATS_USER", "tok"), ("NATS_PASSWORD", "")]);

    assert!(matches!(config.auth, NatsAuth::None));
    assert!(config.token.as_ref().and_then(literal_text) == Some("tok"));
}

#[test]
fn token_variable_applies_alone() {
    let config = config(&[("NATS_TOKEN", "my-token")]);

    assert!(matches!(config.auth, NatsAuth::None));
    assert!(config.token.as_ref().and_then(literal_text) == Some("my-token"));
}

#[test]
fn token_applies_on_top_of_another_method() {
    let config = config(&[("NATS_CREDS", "/c.creds"), ("NATS_TOKEN", "my-token")]);

    assert!(matches!(config.auth, NatsAuth::Credentials(_)));
    assert!(config.token.as_ref().and_then(literal_text) == Some("my-token"));
}

#[test]
fn token_applies_on_top_of_user_password() {
    let config = config(&[("NATS_USER", "u"), ("NATS_PASSWORD", "p"), ("NATS_TOKEN", "t")]);

    assert!(matches!(config.auth, NatsAuth::UserPassword { .. }));
    assert!(config.token.as_ref().and_then(literal_text) == Some("t"));
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
    assert!(matches!(config.token, Some(CredentialValue::File(_))));
}

#[test]
fn unsupported_value_scheme_is_kept_for_connect_time() {
    let config = config(&[("NATS_TOKEN", "op://vault/item/field")]);

    assert!(matches!(config.token, Some(CredentialValue::Unsupported(ref scheme)) if scheme == "op"));
}

#[test]
fn tls_needs_cert_and_key_together() {
    assert!(config(&[("NATS_CERT", "/c.pem")]).tls.client_certificate().is_none());
    assert!(config(&[("NATS_KEY", "/k.pem")]).tls.client_certificate().is_none());

    let both = config(&[("NATS_CERT", "/c.pem"), ("NATS_KEY", "file:///k.pem")]);
    assert!(both.tls.client_certificate().is_some());
}

#[test]
fn tls_ca_is_read() {
    let config = config(&[("NATS_CA", "/ca.pem")]);

    assert_eq!(config.tls.root_ca(), Some(&Location::Path("/ca.pem".into())));
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
        ])
    );

    for secret in ["password", "token", "jwt", "seed"] {
        assert!(!debug.contains(&format!("super-secret-{secret}")));
    }
    assert!(debug.contains("<redacted>"));
}
