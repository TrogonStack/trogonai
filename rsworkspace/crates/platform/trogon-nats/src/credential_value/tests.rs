use super::*;
use trogon_std::env::InMemoryEnv;

fn literal_text(value: &CredentialValue) -> Option<&str> {
    match value {
        CredentialValue::Literal(secret) => Some(secret.as_str()),
        _ => None,
    }
}

#[test]
fn bare_value_is_literal() {
    let value = CredentialValue::parse("hunter2").unwrap();
    assert_eq!(literal_text(&value), Some("hunter2"));
}

#[test]
fn empty_value_is_absent() {
    assert!(CredentialValue::parse("").is_none());
    assert!(Location::parse("").is_none());
}

#[test]
fn jwt_looking_value_is_literal() {
    let jwt = "eyJ0eXAiOiJKV1QiLCJhbGciOiJlZDI1NTE5LW5rZXkifQ.eyJqdGkiOiJYIn0.c2ln-_";
    let value = CredentialValue::parse(jwt).unwrap();
    assert_eq!(literal_text(&value), Some(jwt));
}

#[test]
fn file_scheme_points_at_a_file() {
    assert!(matches!(
        CredentialValue::parse("file:///run/secrets/token"),
        Some(CredentialValue::File(path)) if path == Path::new("/run/secrets/token")
    ));
    assert!(matches!(
        CredentialValue::parse("file://relative/token"),
        Some(CredentialValue::File(path)) if path == Path::new("relative/token")
    ));
}

#[test]
fn env_scheme_names_a_variable() {
    assert!(matches!(
        CredentialValue::parse("env://MY_SECRET"),
        Some(CredentialValue::Env(name)) if name == "MY_SECRET"
    ));
}

#[test]
fn unsupported_schemes_are_flagged() {
    for (raw, scheme) in [
        ("op://vault/item/field", "op"),
        ("nsc://operator/account/user", "nsc"),
        ("data:text/plain;base64,AAAA", "data"),
    ] {
        assert!(matches!(
            CredentialValue::parse(raw),
            Some(CredentialValue::Unsupported(found)) if found == scheme
        ));
    }
}

#[test]
fn unknown_scheme_is_literal() {
    let value = CredentialValue::parse("pass:word").unwrap();
    assert_eq!(literal_text(&value), Some("pass:word"));
}

#[test]
fn single_character_scheme_is_literal() {
    let value = CredentialValue::parse("C:\\secrets\\token").unwrap();
    assert_eq!(literal_text(&value), Some("C:\\secrets\\token"));
}

#[test]
fn location_accepts_bare_and_file_paths() {
    assert_eq!(
        Location::parse("/etc/nats/user.creds"),
        Some(Location::Path("/etc/nats/user.creds".into()))
    );
    assert_eq!(
        Location::parse("file:///etc/nats/user.creds"),
        Some(Location::Path("/etc/nats/user.creds".into()))
    );
}

#[test]
fn location_rejects_other_schemes() {
    for raw in ["op://vault/item", "nsc://a/b/c", "env://X", "data:text/plain,abc"] {
        let location = Location::parse(raw).unwrap();
        assert!(
            matches!(location.path(), Err(SettingError::UnsupportedScheme(_))),
            "{raw}"
        );
    }
}

#[tokio::test]
async fn resolve_literal_returns_value() {
    let value = CredentialValue::parse("abc").unwrap();
    assert_eq!(value.resolve(&InMemoryEnv::new()).await.unwrap(), "abc");
}

#[tokio::test]
async fn resolve_file_trims_trailing_newlines_only() {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), " secret \r\n\n").unwrap();
    let value = CredentialValue::parse(&format!("file://{}", file.path().display())).unwrap();

    assert_eq!(value.resolve(&InMemoryEnv::new()).await.unwrap(), " secret ");
}

#[tokio::test]
async fn resolve_missing_file_reports_path() {
    let value = CredentialValue::parse("file:///nonexistent/secret").unwrap();
    let err = value.resolve(&InMemoryEnv::new()).await.unwrap_err();

    assert!(matches!(err, SettingError::Read { ref path, .. } if path == Path::new("/nonexistent/secret")));
}

#[tokio::test]
async fn resolve_env_reads_named_variable() {
    let env = InMemoryEnv::new();
    env.set("MY_SECRET", "from-env");
    let value = CredentialValue::parse("env://MY_SECRET").unwrap();

    assert_eq!(value.resolve(&env).await.unwrap(), "from-env");
}

#[tokio::test]
async fn resolve_unset_env_fails() {
    let value = CredentialValue::parse("env://MISSING").unwrap();
    let err = value.resolve(&InMemoryEnv::new()).await.unwrap_err();

    assert!(matches!(err, SettingError::EnvUnset(ref name) if name == "MISSING"));
}

#[tokio::test]
async fn resolve_unsupported_fails() {
    let value = CredentialValue::parse("op://vault/item").unwrap();
    let err = value.resolve(&InMemoryEnv::new()).await.unwrap_err();

    assert!(matches!(err, SettingError::UnsupportedScheme(ref scheme) if scheme == "op"));
}

fn seed_file(contents: &str) -> (tempfile::NamedTempFile, Location) {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), contents).unwrap();
    let location = Location::parse(file.path().to_str().unwrap()).unwrap();
    (file, location)
}

#[tokio::test]
async fn location_read_seed_accepts_plain_seed_file() {
    let seed = nkeys::KeyPair::new_user().seed().unwrap();
    let (_file, location) = seed_file(&format!("\n {seed} \n"));

    assert_eq!(location.read_seed().await.unwrap(), seed);
}

#[tokio::test]
async fn location_read_seed_accepts_decorated_creds_file() {
    let seed = nkeys::KeyPair::new_user().seed().unwrap();
    let (_file, location) = seed_file(&format!(
        "-----BEGIN NATS USER JWT-----\neyJ0eXAiOiJKV1QifQ.e30.sig\n------END NATS USER JWT------\n\n-----BEGIN USER NKEY SEED-----\n{seed}\n------END USER NKEY SEED------\n"
    ));

    assert_eq!(location.read_seed().await.unwrap(), seed);
}

#[tokio::test]
async fn location_read_seed_does_not_carry_the_path_on_read_errors() {
    let location = Location::parse("/nonexistent/seed.nk").unwrap();

    assert!(matches!(location.read_seed().await, Err(SettingError::SeedFileRead(_))));
}

#[tokio::test]
async fn location_read_seed_rejects_file_without_seed() {
    let (_file, location) = seed_file("not a seed\n");

    assert!(matches!(location.read_seed().await, Err(SettingError::NoSeedFound)));
}

#[tokio::test]
async fn location_read_seed_rejects_malformed_seed() {
    let (_file, location) = seed_file("SUNOTAVALIDSEED\n");

    assert!(matches!(location.read_seed().await, Err(SettingError::InvalidSeed(_))));
}

#[test]
fn debug_redacts_literals_only() {
    let literal = format!("{:?}", CredentialValue::parse("super-secret").unwrap());
    assert!(!literal.contains("super-secret"));
    assert!(literal.contains("<redacted>"));

    let file = format!("{:?}", CredentialValue::parse("file:///run/token").unwrap());
    assert!(file.contains("/run/token"));
}

#[test]
fn debug_shows_references_by_name() {
    let env = format!("{:?}", CredentialValue::parse("env://NATS_SECRET").unwrap());
    let unsupported = format!("{:?}", CredentialValue::parse("op://vault/item").unwrap());

    assert_eq!(env, r#"Env("NATS_SECRET")"#);
    assert_eq!(unsupported, r#"Unsupported("op")"#);
}

#[test]
fn nkey_seed_debug_redacts_inline_seeds_and_paths() {
    let seed = nkeys::KeyPair::new_user().seed().unwrap();
    let mistyped = format!("{seed}X");
    let inline = format!("{:?}", NKeySeed::parse(&seed).unwrap());
    let file = format!("{:?}", NKeySeed::parse(&mistyped).unwrap());

    assert!(!inline.contains(&seed));
    assert!(inline.contains("<redacted>"));
    assert!(!file.contains(&seed));
    assert!(file.contains("<redacted>"));
}

#[test]
fn missing_companion_names_the_other_setting() {
    assert_eq!(
        SettingError::MissingCompanion("NATS_KEY").to_string(),
        "must be set together with NATS_KEY"
    );
}

#[test]
fn nkey_seed_path_ignores_surrounding_whitespace() {
    let seed = NKeySeed::parse(" /run/user.nk\n").unwrap();

    assert!(matches!(seed, NKeySeed::File(Location::Path(ref path)) if path == Path::new("/run/user.nk")));
}

#[test]
fn nkey_seed_env_scheme_names_a_variable() {
    assert!(matches!(
        NKeySeed::parse("env://NATS_NKEY_SEED"),
        Some(NKeySeed::Env(ref name)) if name == "NATS_NKEY_SEED"
    ));
}

#[test]
fn nkey_seed_debug_shows_env_references_by_name() {
    assert_eq!(
        format!("{:?}", NKeySeed::parse("env://NATS_NKEY_SEED").unwrap()),
        r#"Env("NATS_NKEY_SEED")"#
    );
}

#[tokio::test]
async fn nkey_seed_resolve_env_reads_the_named_variable() {
    let seed = nkeys::KeyPair::new_user().seed().unwrap();
    let env = InMemoryEnv::new();
    env.set("NATS_NKEY_SEED", format!(" {seed}\n"));

    let resolved = NKeySeed::parse("env://NATS_NKEY_SEED").unwrap().resolve(&env).await;

    assert_eq!(resolved.unwrap(), seed);
}

#[tokio::test]
async fn nkey_seed_resolve_unset_env_fails() {
    let err = NKeySeed::parse("env://MISSING")
        .unwrap()
        .resolve(&InMemoryEnv::new())
        .await
        .unwrap_err();

    assert!(matches!(err, SettingError::EnvUnset(ref name) if name == "MISSING"));
}

#[tokio::test]
async fn nkey_seed_resolve_env_rejects_a_malformed_seed() {
    let env = InMemoryEnv::new();
    env.set("NATS_NKEY_SEED", "SUNOTAVALIDSEED");

    let err = NKeySeed::parse("env://NATS_NKEY_SEED")
        .unwrap()
        .resolve(&env)
        .await
        .unwrap_err();

    assert!(matches!(err, SettingError::InvalidSeed(_)));
}
