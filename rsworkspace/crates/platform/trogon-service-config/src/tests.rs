use super::*;
use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{CommandFactory, Parser};
use trogon_nats::{
    ClientCertificate, CredentialValue, IncompleteClientCertificate, Location, NKeySeed, NatsAuth, NatsTls,
    TlsMisconfiguration,
};
use trogon_std::env::InMemoryEnv;

const SECRET_FLAGS: [&str; 5] = [
    "--nats-nkey",
    "--nats-password",
    "--nats-token",
    "--nats-jwt",
    "--nats-seed",
];

fn secret(value: &str) -> SecretString {
    SecretString::new(value).unwrap()
}

fn reference(raw: &str) -> SecretReference {
    raw.parse().unwrap()
}

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

    let resolved = resolve_nats(&section, &NatsArgs::default(), &InMemoryEnv::new()).unwrap();

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
        nats_token: Some(reference("env://OVERRIDE_TOKEN")),
        ..Default::default()
    };

    let resolved = resolve_nats(&section, &overrides, &InMemoryEnv::new()).unwrap();

    assert_eq!(resolved.servers, vec!["override1:4222", "override2:4222"]);
    assert!(matches!(resolved.auth, NatsAuth::Token(CredentialValue::Env(ref name)) if name == "OVERRIDE_TOKEN"));
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
        nats_token: Some(reference("env://OVERRIDE_TOKEN")),
        ..Default::default()
    };

    let resolved = resolve_nats(&section, &overrides, &InMemoryEnv::new()).unwrap();

    assert!(
        matches!(resolved.auth, NatsAuth::Credentials(Location::Path(ref path)) if path == Path::new("/tmp/nats.creds"))
    );
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

    let resolved = resolve_nats(&section, &NatsArgs::default(), &InMemoryEnv::new()).unwrap();

    assert!(matches!(resolved.auth, NatsAuth::NKey(_)));
}

#[test]
fn resolve_nats_token_beats_user_without_password() {
    let section = NatsConfigSection {
        user: Some("as-token".to_string()),
        token: Some(secret("section-token")),
        ..empty_section()
    };

    let resolved = resolve_nats(&section, &NatsArgs::default(), &InMemoryEnv::new()).unwrap();

    assert!(
        matches!(resolved.auth, NatsAuth::Token(CredentialValue::Literal(ref token)) if token.as_str() == "section-token")
    );
}

#[test]
fn resolve_nats_nkey_accepts_a_seed_file_location() {
    let overrides = NatsArgs {
        nats_nkey: Some(reference("file:///seed.nk")),
        ..Default::default()
    };

    let resolved = resolve_nats(&empty_section(), &overrides, &InMemoryEnv::new()).unwrap();

    assert!(
        matches!(resolved.auth, NatsAuth::NKey(NKeySeed::File(Location::Path(ref path))) if path == Path::new("/seed.nk"))
    );
}

#[test]
fn resolve_nats_nkey_accepts_an_env_reference() {
    let overrides = NatsArgs {
        nats_nkey: Some(reference("env://NKEY_SEED")),
        ..Default::default()
    };

    let resolved = resolve_nats(&empty_section(), &overrides, &InMemoryEnv::new()).unwrap();

    assert!(matches!(resolved.auth, NatsAuth::NKey(NKeySeed::Env(ref name)) if name == "NKEY_SEED"));
}

#[test]
fn resolve_nats_jwt_and_seed_from_section_and_overrides() {
    let section = NatsConfigSection {
        jwt: Some(secret("section-jwt")),
        ..empty_section()
    };
    let overrides = NatsArgs {
        nats_seed: Some(reference("env://OVERRIDE_SEED")),
        ..Default::default()
    };

    let resolved = resolve_nats(&section, &overrides, &InMemoryEnv::new()).unwrap();

    assert!(matches!(
        resolved.auth,
        NatsAuth::Jwt { jwt: CredentialValue::Literal(ref jwt), seed: Some(CredentialValue::Env(ref seed)) }
            if jwt.as_str() == "section-jwt" && seed == "OVERRIDE_SEED"
    ));
}

#[test]
fn resolve_nats_jwt_accepts_a_file_reference() {
    let overrides = NatsArgs {
        nats_jwt: Some(reference("file:///run/secrets/nats.jwt")),
        ..Default::default()
    };

    let resolved = resolve_nats(&empty_section(), &overrides, &InMemoryEnv::new()).unwrap();

    assert!(matches!(
        resolved.auth,
        NatsAuth::Jwt { jwt: CredentialValue::File(ref path), seed: None } if path == Path::new("/run/secrets/nats.jwt")
    ));
}

#[test]
fn resolve_nats_rejects_a_cli_user_that_would_be_sent_as_a_token() {
    let overrides = NatsArgs {
        nats_user: Some("would-be-token".to_string()),
        ..Default::default()
    };

    assert_eq!(
        resolve_nats(&empty_section(), &overrides, &InMemoryEnv::new()).err(),
        Some(NatsArgsError::UserSentAsToken)
    );
}

#[test]
fn resolve_nats_accepts_a_cli_user_with_a_section_password() {
    let section = NatsConfigSection {
        password: Some(secret("section-password")),
        ..empty_section()
    };
    let overrides = NatsArgs {
        nats_user: Some("alice".to_string()),
        ..Default::default()
    };

    let resolved = resolve_nats(&section, &overrides, &InMemoryEnv::new()).unwrap();

    assert!(matches!(resolved.auth, NatsAuth::UserPassword { ref user, .. } if user == "alice"));
}

#[test]
fn resolve_nats_accepts_a_cli_user_with_a_password_reference() {
    let overrides = NatsArgs {
        nats_user: Some("alice".to_string()),
        nats_password: Some(reference("file:///run/secrets/nats-password")),
        ..Default::default()
    };

    let resolved = resolve_nats(&empty_section(), &overrides, &InMemoryEnv::new()).unwrap();

    assert!(matches!(
        resolved.auth,
        NatsAuth::UserPassword { ref user, password: CredentialValue::File(ref path) }
            if user == "alice" && path == Path::new("/run/secrets/nats-password")
    ));
}

#[test]
fn resolve_nats_ignores_a_cli_user_when_a_token_is_set() {
    let section = NatsConfigSection {
        token: Some(secret("section-token")),
        ..empty_section()
    };
    let overrides = NatsArgs {
        nats_user: Some("alice".to_string()),
        ..Default::default()
    };

    let resolved = resolve_nats(&section, &overrides, &InMemoryEnv::new()).unwrap();

    assert!(
        matches!(resolved.auth, NatsAuth::Token(CredentialValue::Literal(ref token)) if token.as_str() == "section-token")
    );
}

#[test]
fn resolve_nats_ignores_a_cli_user_when_creds_win() {
    let overrides = NatsArgs {
        nats_creds: Some("/tmp/nats.creds".to_string()),
        nats_user: Some("alice".to_string()),
        ..Default::default()
    };

    let resolved = resolve_nats(&empty_section(), &overrides, &InMemoryEnv::new()).unwrap();

    assert!(matches!(resolved.auth, NatsAuth::Credentials(_)));
}

#[test]
fn resolve_nats_keeps_a_section_user_without_password_as_a_token() {
    let section = NatsConfigSection {
        user: Some("section-user-token".to_string()),
        ..empty_section()
    };

    let resolved = resolve_nats(&section, &NatsArgs::default(), &InMemoryEnv::new()).unwrap();

    assert!(
        matches!(resolved.auth, NatsAuth::Token(CredentialValue::Literal(ref token)) if token.as_str() == "section-user-token")
    );
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

    let resolved = resolve_nats(&section, &overrides, &InMemoryEnv::new()).unwrap();

    assert_eq!(
        resolved.tls,
        Ok(NatsTls::Enabled {
            ca: Some(Location::Path("/override-ca.pem".into())),
            client_cert: Some(ClientCertificate {
                cert: Location::Path("/override.crt".into()),
                key: Location::Path("/section.key".into()),
            }),
        })
    );
}

#[test]
fn resolve_nats_tls_needs_cert_and_key() {
    let section = NatsConfigSection {
        tls_cert: Some("/only.crt".to_string()),
        ..empty_section()
    };

    assert_eq!(
        resolve_nats(&section, &NatsArgs::default(), &InMemoryEnv::new())
            .unwrap()
            .tls,
        Err(TlsMisconfiguration::IncompleteClientCertificate(
            IncompleteClientCertificate::MissingKey
        ))
    );
}

#[test]
fn resolve_nats_keeps_tls_on_unless_the_environment_disables_it() {
    let env = InMemoryEnv::new();
    env.set("NATS_TLS_DISABLED", "true");

    assert_eq!(
        resolve_nats(&empty_section(), &NatsArgs::default(), &InMemoryEnv::new())
            .unwrap()
            .tls,
        Ok(NatsTls::default())
    );
    assert_eq!(
        resolve_nats(&empty_section(), &NatsArgs::default(), &env).unwrap().tls,
        Ok(NatsTls::Disabled)
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

#[derive(Parser)]
struct NatsCli {
    #[command(flatten)]
    nats: NatsArgs,
}

#[test]
fn nats_args_accept_references_for_every_secret_flag() {
    let cli = NatsCli::try_parse_from([
        "cli",
        "--nats-nkey",
        "file:///run/secrets/nats.nk",
        "--nats-password",
        "env://NATS_PASSWORD_VALUE",
        "--nats-token",
        "env://NATS_TOKEN_VALUE",
        "--nats-jwt",
        "file:///run/secrets/nats.jwt",
        "--nats-seed",
        "env://NATS_SEED_VALUE",
    ])
    .unwrap();

    assert_eq!(cli.nats.nats_nkey, Some(reference("file:///run/secrets/nats.nk")));
    assert_eq!(cli.nats.nats_password, Some(reference("env://NATS_PASSWORD_VALUE")));
    assert_eq!(cli.nats.nats_token, Some(reference("env://NATS_TOKEN_VALUE")));
    assert_eq!(cli.nats.nats_jwt, Some(reference("file:///run/secrets/nats.jwt")));
    assert_eq!(cli.nats.nats_seed, Some(reference("env://NATS_SEED_VALUE")));
}

#[test]
fn nats_args_reject_literal_secrets_for_every_secret_flag() {
    for flag in SECRET_FLAGS {
        let err = NatsCli::try_parse_from(["cli", flag, "literal-secret-value"])
            .err()
            .unwrap();

        assert_eq!(err.kind(), ErrorKind::ValueValidation, "{flag}");
        assert_eq!(
            err.get(ContextKind::InvalidArg),
            Some(&ContextValue::String(format!("{flag} <REFERENCE>"))),
            "{flag}"
        );
        assert_eq!(err.get(ContextKind::InvalidValue), None, "{flag}");
    }
}

#[test]
fn nats_args_help_offers_no_flag_that_takes_a_secret() {
    let help = NatsCli::command().render_long_help().to_string();

    for flag in SECRET_FLAGS {
        assert!(help.contains(&format!("{flag} <REFERENCE>")), "{help}");
    }
}
