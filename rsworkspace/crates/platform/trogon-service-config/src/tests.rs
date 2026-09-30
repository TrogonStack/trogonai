use super::*;

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
    };

    let resolved = resolve_nats(&section, &NatsArgs::default());

    assert_eq!(resolved.servers, vec!["host1:4222", "host2:4222"]);
    assert!(matches!(resolved.auth, NatsAuth::Token(ref token) if token == "section-token"));
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
    };
    let overrides = NatsArgs {
        nats_url: Some("override1:4222,override2:4222".to_string()),
        nats_token: Some("override-token".to_string()),
        ..Default::default()
    };

    let resolved = resolve_nats(&section, &overrides);

    assert_eq!(resolved.servers, vec!["override1:4222", "override2:4222"]);
    assert!(matches!(resolved.auth, NatsAuth::Token(ref token) if token == "override-token"));
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
    };
    let overrides = NatsArgs {
        nats_creds: Some("/tmp/nats.creds".to_string()),
        nats_token: Some("override-token".to_string()),
        ..Default::default()
    };

    let resolved = resolve_nats(&section, &overrides);

    assert!(matches!(resolved.auth, NatsAuth::Credentials(ref path) if path == Path::new("/tmp/nats.creds")));
}

#[test]
fn nats_args_debug_redacts_secrets() {
    let args = NatsArgs {
        nats_nkey: Some("super-secret-nkey".to_string()),
        nats_password: Some("super-secret-password".to_string()),
        nats_token: Some("super-secret-token".to_string()),
        ..Default::default()
    };
    let debug_output = format!("{args:?}");
    assert!(!debug_output.contains("super-secret-nkey"));
    assert!(!debug_output.contains("super-secret-password"));
    assert!(!debug_output.contains("super-secret-token"));
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
    };
    let debug_output = format!("{section:?}");
    assert!(!debug_output.contains("super-secret-nkey"));
    assert!(!debug_output.contains("super-secret-password"));
    assert!(!debug_output.contains("super-secret-token"));
    assert!(debug_output.contains("alice"));
    assert!(debug_output.contains("<redacted>"));
}

#[test]
fn load_config_reads_optional_file() {
    let file = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    std::fs::write(file.path(), "value = 'from-file'\n").unwrap();

    let loaded: DummyConfig = load_config(Some(file.path())).unwrap();

    assert_eq!(loaded.value, "from-file");
}
