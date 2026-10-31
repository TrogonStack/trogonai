use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use trogon_std::env::InMemoryEnv;

use super::*;

fn jwt(claims: &str) -> String {
    let encode = &base64::engine::general_purpose::URL_SAFE_NO_PAD;
    format!(
        "{}.{}.{}",
        encode.encode(br#"{"alg":"HS256","typ":"JWT"}"#),
        encode.encode(claims),
        encode.encode(b"test-signature")
    )
}

fn in_an_hour() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + 3600
}

fn caller_jwt(expiration: u64) -> String {
    jwt(&format!(
        r#"{{"exp":{expiration},"caller_id":"alice","principal_kind":"person"}}"#
    ))
}

fn env_with_caller(caller_jwt: &str) -> InMemoryEnv {
    let env = InMemoryEnv::new();
    env.set(ENV_A2A_AGENT_ID, "bot");
    env.set(ENV_A2A_CALLER_JWT, caller_jwt);
    env
}

#[test]
fn runtime_error_display_missing_agent_id() {
    assert_eq!(
        RuntimeError::MissingAgentId.to_string(),
        "A2A_AGENT_ID environment variable is required"
    );
}

#[test]
fn runtime_error_display_invalid_prefix() {
    let e = RuntimeError::InvalidPrefix(A2aPrefix::new("").unwrap_err());
    assert_eq!(e.to_string(), "invalid A2A prefix");
    assert!(std::error::Error::source(&e).is_some());
}

#[test]
fn runtime_error_display_invalid_agent_id() {
    let e = RuntimeError::InvalidAgentId(A2aAgentId::new("a.b").unwrap_err());
    assert_eq!(e.to_string(), "invalid agent id");
    assert!(std::error::Error::source(&e).is_some());
}

#[test]
fn runtime_error_display_and_source_for_nats_connect() {
    let inner = ConnectError::InvalidCredentials(std::io::Error::other("oops"));
    let e = RuntimeError::NatsConnect(inner);
    assert_eq!(e.to_string(), "NATS connection failed");
    assert!(std::error::Error::source(&e).is_some());
}

#[test]
fn runtime_error_missing_agent_id_has_no_source() {
    let e = RuntimeError::MissingAgentId;
    assert!(std::error::Error::source(&e).is_none());
}

#[test]
fn parse_env_missing_agent_id_returns_error() {
    let env = InMemoryEnv::new();
    let result = parse_env(&env);
    assert!(matches!(result, Err(RuntimeError::MissingAgentId)));
}

#[test]
fn parse_env_invalid_agent_id_returns_error() {
    let env = InMemoryEnv::new();
    env.set(ENV_A2A_AGENT_ID, "a.b");
    let result = parse_env(&env);
    assert!(matches!(result, Err(RuntimeError::InvalidAgentId(_))));
}

#[test]
fn parse_env_invalid_prefix_returns_error() {
    let env = InMemoryEnv::new();
    env.set(ENV_A2A_PREFIX, "bad prefix!");
    env.set(ENV_A2A_AGENT_ID, "bot");
    let result = parse_env(&env);
    assert!(matches!(result, Err(RuntimeError::InvalidPrefix(_))));
}

#[test]
fn parse_env_valid_uses_default_prefix() {
    let env = env_with_caller(&caller_jwt(in_an_hour()));
    let cfg = parse_env(&env).expect("valid");
    assert_eq!(cfg.prefix.as_str(), DEFAULT_A2A_PREFIX);
    assert_eq!(cfg.agent_id.as_str(), "bot");
    assert_eq!(cfg.caller, SubjectPrincipal::from_tokens("person", "alice").unwrap());
}

#[test]
fn parse_env_requires_a_caller_jwt() {
    let env = InMemoryEnv::new();
    env.set(ENV_A2A_AGENT_ID, "bot");
    assert!(matches!(parse_env(&env), Err(RuntimeError::MissingCallerJwt)));
    assert_eq!(
        RuntimeError::MissingCallerJwt.to_string(),
        "A2A_CALLER_JWT environment variable is required"
    );
}

#[test]
fn parse_env_rejects_a_malformed_or_expired_caller_jwt() {
    for raw in ["malformed".to_owned(), caller_jwt(1)] {
        let error = parse_env(&env_with_caller(&raw)).err().expect("rejected");
        assert!(matches!(error, RuntimeError::InvalidCallerJwt(_)), "{error:?}");
        assert_eq!(error.to_string(), "invalid caller JWT");
        assert!(std::error::Error::source(&error).is_some());
    }
}

#[test]
fn parse_env_rejects_a_caller_jwt_that_names_no_principal() {
    let expiration = in_an_hour();
    for claims in [
        format!(r#"{{"exp":{expiration},"caller_id":"alice"}}"#),
        format!(r#"{{"exp":{expiration},"caller_id":"alice","principal_kind":"robot"}}"#),
        format!(r#"{{"exp":{expiration},"principal_kind":"person"}}"#),
    ] {
        let error = parse_env(&env_with_caller(&jwt(&claims))).err().expect("rejected");
        assert!(matches!(error, RuntimeError::InvalidCallerPrincipal(_)), "{error:?}");
        assert_eq!(error.to_string(), "caller JWT names no caller principal");
        assert!(std::error::Error::source(&error).is_some());
    }
}

#[test]
fn runtime_error_display_and_source_for_io_loop() {
    let e = RuntimeError::IoLoop(std::io::Error::other("nope"));
    assert_eq!(e.to_string(), "stdio loop failed");
    assert!(std::error::Error::source(&e).is_some());
}

#[test]
fn from_impls_construct_variants() {
    let e: RuntimeError = A2aPrefix::new("").unwrap_err().into();
    assert!(matches!(e, RuntimeError::InvalidPrefix(_)));
    let e: RuntimeError = A2aAgentId::new("a.b").unwrap_err().into();
    assert!(matches!(e, RuntimeError::InvalidAgentId(_)));
    let e: RuntimeError = ConnectError::InvalidCredentials(std::io::Error::other("x")).into();
    assert!(matches!(e, RuntimeError::NatsConnect(_)));
}
