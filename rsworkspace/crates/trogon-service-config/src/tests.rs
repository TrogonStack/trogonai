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
fn load_config_reads_optional_file() {
    let file = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    std::fs::write(file.path(), "value = 'from-file'\n").unwrap();

    let loaded: DummyConfig = load_config(Some(file.path())).unwrap();

    assert_eq!(loaded.value, "from-file");
}

#[test]
fn load_config_with_no_path_uses_default() {
    let loaded: DummyConfig = load_config(None).unwrap();
    assert_eq!(loaded.value, "default");
}

#[test]
fn resolve_nats_uses_nkey_auth() {
    let section = NatsConfigSection {
        url: "host:4222".to_string(),
        creds: None,
        nkey: Some("SUANQQX".to_string()),
        user: None,
        password: None,
        token: None,
    };
    let resolved = resolve_nats(&section, &NatsArgs::default());
    assert!(
        matches!(resolved.auth, NatsAuth::NKey(ref k) if k == "SUANQQX"),
        "expected NKey auth, got {:?}",
        resolved.auth
    );
}

#[test]
fn resolve_nats_nkey_override_beats_section_token() {
    let section = NatsConfigSection {
        url: "host:4222".to_string(),
        creds: None,
        nkey: None,
        user: None,
        password: None,
        token: Some("section-token".to_string()),
    };
    let overrides = NatsArgs {
        nats_nkey: Some("override-nkey".to_string()),
        ..Default::default()
    };
    let resolved = resolve_nats(&section, &overrides);
    assert!(
        matches!(resolved.auth, NatsAuth::NKey(ref k) if k == "override-nkey"),
        "expected NKey override, got {:?}",
        resolved.auth
    );
}

#[test]
fn resolve_nats_uses_user_password_auth() {
    let section = NatsConfigSection {
        url: "host:4222".to_string(),
        creds: None,
        nkey: None,
        user: Some("alice".to_string()),
        password: Some("secret".to_string()),
        token: None,
    };
    let resolved = resolve_nats(&section, &NatsArgs::default());
    assert!(
        matches!(resolved.auth, NatsAuth::UserPassword { ref user, ref password }
            if user == "alice" && password == "secret"),
        "expected UserPassword auth, got {:?}",
        resolved.auth
    );
}

#[test]
fn resolve_nats_user_password_override_beats_section_token() {
    let section = NatsConfigSection {
        url: "host:4222".to_string(),
        creds: None,
        nkey: None,
        user: None,
        password: None,
        token: Some("section-token".to_string()),
    };
    let overrides = NatsArgs {
        nats_user: Some("bob".to_string()),
        nats_password: Some("pass123".to_string()),
        ..Default::default()
    };
    let resolved = resolve_nats(&section, &overrides);
    assert!(
        matches!(resolved.auth, NatsAuth::UserPassword { ref user, ref password }
            if user == "bob" && password == "pass123"),
        "expected UserPassword override, got {:?}",
        resolved.auth
    );
}

#[test]
fn resolve_nats_no_auth_when_all_empty() {
    let section = NatsConfigSection {
        url: "host:4222".to_string(),
        creds: None,
        nkey: None,
        user: None,
        password: None,
        token: None,
    };
    let resolved = resolve_nats(&section, &NatsArgs::default());
    assert!(
        matches!(resolved.auth, NatsAuth::None),
        "expected NatsAuth::None, got {:?}",
        resolved.auth
    );
}

#[test]
fn resolve_nats_empty_string_override_falls_back_to_section() {
    let section = NatsConfigSection {
        url: "host:4222".to_string(),
        creds: None,
        nkey: None,
        user: None,
        password: None,
        token: Some("section-token".to_string()),
    };
    let overrides = NatsArgs {
        nats_token: Some(String::new()),
        ..Default::default()
    };
    let resolved = resolve_nats(&section, &overrides);
    assert!(
        matches!(resolved.auth, NatsAuth::Token(ref t) if t == "section-token"),
        "empty string override must fall back to section value, got {:?}",
        resolved.auth
    );
}

#[test]
fn resolve_nats_empty_url_override_falls_back_to_section() {
    let section = NatsConfigSection {
        url: "section-host:4222".to_string(),
        creds: None,
        nkey: None,
        user: None,
        password: None,
        token: None,
    };
    let overrides = NatsArgs {
        nats_url: Some(String::new()),
        ..Default::default()
    };
    let resolved = resolve_nats(&section, &overrides);
    assert_eq!(resolved.servers, vec!["section-host:4222"]);
}

#[test]
fn resolve_nats_creds_beats_nkey_beats_user_password_beats_token() {
    let section = NatsConfigSection {
        url: "host:4222".to_string(),
        creds: Some("/path/to/creds".to_string()),
        nkey: Some("some-nkey".to_string()),
        user: Some("user".to_string()),
        password: Some("pass".to_string()),
        token: Some("tok".to_string()),
    };
    let resolved = resolve_nats(&section, &NatsArgs::default());
    assert!(
        matches!(resolved.auth, NatsAuth::Credentials(_)),
        "creds must win when multiple auth methods are set, got {:?}",
        resolved.auth
    );
}
