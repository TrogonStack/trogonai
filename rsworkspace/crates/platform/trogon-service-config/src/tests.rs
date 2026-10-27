use super::*;
use trogon_nats::{CredentialValue, Location, NKeySeed, NatsAuth};

#[derive(Config)]
struct DummyConfig {
    #[config(env = "DUMMY_VALUE", default = "default")]
    value: String,
}

#[test]
fn resolve_nats_uses_section_when_no_overrides() {
    let section = NatsConfigSection {
        url: "host1:4222, host2:4222".to_string(),
        creds: None,
        nkey: None,
        user: None,
        password: None,
        token: Some("section-token".to_string()),
        jwt: None,
        seed: None,
        tls_cert: None,
        tls_key: None,
        tls_ca: None,
    };

    let resolved = resolve_nats(&section, &NatsArgs::default());

    assert_eq!(resolved.servers, vec!["host1:4222", "host2:4222"]);
    assert!(
        matches!(resolved.auth, NatsAuth::Token(CredentialValue::Literal(ref token)) if token.as_str() == "section-token")
    );
}

#[test]
fn resolve_nats_prefers_cli_overrides() {
    let section = NatsConfigSection {
        url: "host1:4222".to_string(),
        creds: None,
        nkey: None,
        user: None,
        password: None,
        token: Some("section-token".to_string()),
        jwt: None,
        seed: None,
        tls_cert: None,
        tls_key: None,
        tls_ca: None,
    };
    let overrides = NatsArgs {
        nats_url: Some("override1:4222,override2:4222".to_string()),
        nats_token: Some("override-token".to_string()),
        ..Default::default()
    };

    let resolved = resolve_nats(&section, &overrides);

    assert_eq!(resolved.servers, vec!["override1:4222", "override2:4222"]);
    assert!(
        matches!(resolved.auth, NatsAuth::Token(CredentialValue::Literal(ref token)) if token.as_str() == "override-token")
    );
}

#[test]
fn resolve_nats_keeps_auth_priority_with_overrides() {
    let section = NatsConfigSection {
        url: "host1:4222".to_string(),
        creds: None,
        nkey: None,
        user: None,
        password: None,
        token: Some("section-token".to_string()),
        jwt: None,
        seed: None,
        tls_cert: None,
        tls_key: None,
        tls_ca: None,
    };
    let overrides = NatsArgs {
        nats_creds: Some("/tmp/nats.creds".to_string()),
        nats_token: Some("override-token".to_string()),
        ..Default::default()
    };

    let resolved = resolve_nats(&section, &overrides);

    assert!(
        matches!(resolved.auth, NatsAuth::Credentials(Location::Path(ref path)) if path == Path::new("/tmp/nats.creds"))
    );
}

#[test]
fn nats_args_debug_redacts_secrets() {
    let args = NatsArgs {
        nats_nkey: Some("super-secret-nkey".to_string()),
        nats_password: Some("super-secret-password".to_string()),
        nats_token: Some("super-secret-token".to_string()),
        nats_jwt: Some("super-secret-jwt".to_string()),
        nats_seed: Some("super-secret-seed".to_string()),
        ..Default::default()
    };
    let debug_output = format!("{args:?}");
    assert!(!debug_output.contains("super-secret-nkey"));
    assert!(!debug_output.contains("super-secret-password"));
    assert!(!debug_output.contains("super-secret-token"));
    assert!(!debug_output.contains("super-secret-jwt"));
    assert!(!debug_output.contains("super-secret-seed"));
    assert!(debug_output.contains("<redacted>"));
}

#[test]
fn nats_config_section_debug_redacts_secrets() {
    let section = NatsConfigSection {
        url: "localhost:4222".to_string(),
        creds: None,
        nkey: Some("super-secret-nkey".to_string()),
        user: Some("alice".to_string()),
        password: Some("super-secret-password".to_string()),
        token: Some("super-secret-token".to_string()),
        jwt: Some("super-secret-jwt".to_string()),
        seed: Some("super-secret-seed".to_string()),
        tls_cert: None,
        tls_key: None,
        tls_ca: None,
    };
    let debug_output = format!("{section:?}");
    assert!(!debug_output.contains("super-secret-nkey"));
    assert!(!debug_output.contains("super-secret-password"));
    assert!(!debug_output.contains("super-secret-token"));
    assert!(!debug_output.contains("super-secret-jwt"));
    assert!(!debug_output.contains("super-secret-seed"));
    assert!(debug_output.contains("alice"));
    assert!(debug_output.contains("<redacted>"));
}

fn empty_section() -> NatsConfigSection {
    NatsConfigSection {
        url: "localhost:4222".to_string(),
        creds: None,
        nkey: None,
        user: None,
        password: None,
        token: None,
        jwt: None,
        seed: None,
        tls_cert: None,
        tls_key: None,
        tls_ca: None,
    }
}

#[test]
fn resolve_nats_nkey_beats_user_password() {
    let section = NatsConfigSection {
        nkey: Some("/seed.nk".to_string()),
        user: Some("alice".to_string()),
        password: Some("pw".to_string()),
        ..empty_section()
    };

    let resolved = resolve_nats(&section, &NatsArgs::default());

    assert!(matches!(resolved.auth, NatsAuth::NKey(_)));
}

#[test]
fn resolve_nats_token_beats_user_without_password() {
    let section = NatsConfigSection {
        user: Some("as-token".to_string()),
        token: Some("section-token".to_string()),
        ..empty_section()
    };

    let resolved = resolve_nats(&section, &NatsArgs::default());

    assert!(
        matches!(resolved.auth, NatsAuth::Token(CredentialValue::Literal(ref token)) if token.as_str() == "section-token")
    );
}

#[test]
fn resolve_nats_nkey_accepts_a_seed_file_location() {
    let overrides = NatsArgs {
        nats_nkey: Some("file:///seed.nk".to_string()),
        ..Default::default()
    };

    let resolved = resolve_nats(&empty_section(), &overrides);

    assert!(
        matches!(resolved.auth, NatsAuth::NKey(NKeySeed::File(Location::Path(ref path))) if path == Path::new("/seed.nk"))
    );
}

#[test]
fn resolve_nats_jwt_and_seed_from_section_and_overrides() {
    let section = NatsConfigSection {
        jwt: Some("section-jwt".to_string()),
        ..empty_section()
    };
    let overrides = NatsArgs {
        nats_seed: Some("override-seed".to_string()),
        ..Default::default()
    };

    let resolved = resolve_nats(&section, &overrides);

    assert!(matches!(
        resolved.auth,
        NatsAuth::Jwt { jwt: CredentialValue::Literal(ref jwt), seed: Some(CredentialValue::Literal(ref seed)) }
            if jwt.as_str() == "section-jwt" && seed.as_str() == "override-seed"
    ));
}

#[test]
fn resolve_nats_tls_overrides_section() {
    let section = NatsConfigSection {
        tls_cert: Some("/section.crt".to_string()),
        tls_key: Some("/section.key".to_string()),
        tls_ca: Some("/section-ca.pem".to_string()),
        ..empty_section()
    };
    let overrides = NatsArgs {
        nats_tls_cert: Some("/override.crt".to_string()),
        nats_tls_ca: Some("file:///override-ca.pem".to_string()),
        ..Default::default()
    };

    let resolved = resolve_nats(&section, &overrides);
    let client = resolved.tls.client_certificate().unwrap();

    assert_eq!(client.cert, Location::Path("/override.crt".into()));
    assert_eq!(client.key, Location::Path("/section.key".into()));
    assert_eq!(resolved.tls.root_ca(), Some(&Location::Path("/override-ca.pem".into())));
}

#[test]
fn resolve_nats_tls_needs_cert_and_key() {
    let section = NatsConfigSection {
        tls_cert: Some("/only.crt".to_string()),
        ..empty_section()
    };

    assert!(
        resolve_nats(&section, &NatsArgs::default())
            .tls
            .client_certificate()
            .is_none()
    );
}

#[test]
fn load_config_reads_optional_file() {
    let file = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    std::fs::write(file.path(), "value = 'from-file'\n").unwrap();

    let loaded: DummyConfig = load_config(Some(file.path())).unwrap();

    assert_eq!(loaded.value, "from-file");
}
