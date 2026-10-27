use super::*;

fn secret(value: &str) -> SecretString {
    SecretString::new(value).unwrap()
}
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
        token: Some(secret("section-token")),
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
        token: Some(secret("section-token")),
        jwt: None,
        seed: None,
        tls_cert: None,
        tls_key: None,
        tls_ca: None,
    };
    let overrides = NatsArgs {
        nats_url: Some("override1:4222,override2:4222".to_string()),
        nats_token: Some(secret("override-token")),
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
        token: Some(secret("section-token")),
        jwt: None,
        seed: None,
        tls_cert: None,
        tls_key: None,
        tls_ca: None,
    };
    let overrides = NatsArgs {
        nats_creds: Some("/tmp/nats.creds".to_string()),
        nats_token: Some(secret("override-token")),
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
        nats_nkey: Some(secret("super-secret-nkey")),
        nats_password: Some(secret("super-secret-password")),
        nats_token: Some(secret("super-secret-token")),
        nats_jwt: Some(secret("super-secret-jwt")),
        nats_seed: Some(secret("super-secret-seed")),
        ..Default::default()
    };
    let debug_output = format!("{args:?}");
    assert!(!debug_output.contains("super-secret-nkey"));
    assert!(!debug_output.contains("super-secret-password"));
    assert!(!debug_output.contains("super-secret-token"));
    assert!(!debug_output.contains("super-secret-jwt"));
    assert!(!debug_output.contains("super-secret-seed"));
    assert!(debug_output.contains("SecretString(***)"));
}

#[test]
fn nats_config_section_debug_redacts_secrets() {
    let section = NatsConfigSection {
        url: "localhost:4222".to_string(),
        creds: None,
        nkey: Some(secret("super-secret-nkey")),
        user: Some("alice".to_string()),
        password: Some(secret("super-secret-password")),
        token: Some(secret("super-secret-token")),
        jwt: Some(secret("super-secret-jwt")),
        seed: Some(secret("super-secret-seed")),
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
    assert!(debug_output.contains("SecretString(***)"));
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
        nkey: Some(secret("/seed.nk")),
        user: Some("alice".to_string()),
        password: Some(secret("pw")),
        ..empty_section()
    };

    let resolved = resolve_nats(&section, &NatsArgs::default());

    assert!(matches!(resolved.auth, NatsAuth::NKey(_)));
}

#[test]
fn resolve_nats_token_beats_user_without_password() {
    let section = NatsConfigSection {
        user: Some("as-token".to_string()),
        token: Some(secret("section-token")),
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
        nats_nkey: Some(secret("file:///seed.nk")),
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
        jwt: Some(secret("section-jwt")),
        ..empty_section()
    };
    let overrides = NatsArgs {
        nats_seed: Some(secret("override-seed")),
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
    let client = resolved.tls.client_certificate().unwrap().unwrap();

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

    assert_eq!(
        resolve_nats(&section, &NatsArgs::default()).tls.client_certificate(),
        Err(trogon_nats::IncompleteClientCertificate::MissingKey)
    );
}

#[test]
fn load_config_reads_optional_file() {
    let file = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    std::fs::write(file.path(), "value = 'from-file'\n").unwrap();

    let loaded: DummyConfig = load_config(Some(file.path())).unwrap();

    assert_eq!(loaded.value, "from-file");
}

#[derive(Config)]
struct NatsOnlyConfig {
    #[config(nested)]
    nats: NatsConfigSection,
}

#[test]
fn load_config_reads_nats_secrets_from_file() {
    let file = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    std::fs::write(file.path(), "[nats]\ntoken = 'file-token'\n").unwrap();

    let loaded: NatsOnlyConfig = load_config(Some(file.path())).unwrap();

    assert_eq!(loaded.nats.token.unwrap().as_str(), "file-token");
}

#[derive(clap::Parser)]
struct NatsCli {
    #[command(flatten)]
    nats: NatsArgs,
}

#[test]
fn nats_args_parse_secrets_from_flags() {
    use clap::Parser;

    let cli = NatsCli::try_parse_from(["cli", "--nats-token", "flag-token", "--nats-seed", "flag-seed"]).unwrap();

    assert_eq!(cli.nats.nats_token.unwrap().as_str(), "flag-token");
    assert_eq!(cli.nats.nats_seed.unwrap().as_str(), "flag-seed");
}
