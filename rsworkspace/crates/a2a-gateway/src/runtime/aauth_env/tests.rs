use std::io::Write;

use p256::ecdsa::SigningKey;
use p256::pkcs8::EncodePrivateKey;
use rand_core::OsRng;
use tempfile::NamedTempFile;
use trogon_aauth_verify::TokenError;
use trogon_aauth_verify::nats_pop::NatsPopError;
use trogon_std::env::InMemoryEnv;

use super::*;
use crate::aauth::AAuthDenyReason;

fn write_temp_file(contents: &str) -> NamedTempFile {
    let mut file = NamedTempFile::new().expect("tempfile");
    file.write_all(contents.as_bytes()).expect("write tempfile");
    file
}

fn ec_pem() -> String {
    let sk = SigningKey::random(&mut OsRng);
    sk.to_pkcs8_pem(p256::pkcs8::LineEnding::LF)
        .expect("pkcs8 pem")
        .to_string()
}

fn set_all_required(env: &InMemoryEnv, jwks_path: &str, challenge_key_path: &str) {
    env.set(ENV_AAUTH_JWKS_PATH, jwks_path);
    env.set(ENV_AAUTH_RESOURCE_ISS, "https://resource.test");
    env.set(ENV_AAUTH_PERSON_SERVER_AUD, "https://ps.test");
    env.set(ENV_AAUTH_CHALLENGE_KID, "gw-kid");
    env.set(ENV_AAUTH_CHALLENGE_KEY_PATH, challenge_key_path);
}

#[test]
fn off_mode_is_default_when_unset() {
    let env = InMemoryEnv::new();
    assert!(matches!(gateway_aauth_from_env(&env), Ok(None)));
}

#[test]
fn off_mode_explicit() {
    let env = InMemoryEnv::new();
    env.set(ENV_AAUTH_MODE, "off");
    assert!(matches!(gateway_aauth_from_env(&env), Ok(None)));
}

#[test]
fn invalid_mode_string_errors() {
    let env = InMemoryEnv::new();
    env.set(ENV_AAUTH_MODE, "bogus");
    assert!(matches!(
        gateway_aauth_from_env(&env),
        Err(AAuthEnvError::InvalidMode(_))
    ));
}

#[test]
fn missing_jwks_source_errors_when_neither_path_nor_discovery_set() {
    let env = InMemoryEnv::new();
    env.set(ENV_AAUTH_MODE, "shadow");
    env.set(ENV_AAUTH_RESOURCE_ISS, "https://resource.test");
    env.set(ENV_AAUTH_PERSON_SERVER_AUD, "https://ps.test");
    env.set(ENV_AAUTH_CHALLENGE_KID, "gw-kid");
    env.set(ENV_AAUTH_CHALLENGE_KEY_PATH, "/tmp/does-not-matter");
    assert!(matches!(
        gateway_aauth_from_env(&env),
        Err(AAuthEnvError::MissingJwksSource)
    ));
}

#[test]
fn missing_resource_iss_errors() {
    let env = InMemoryEnv::new();
    env.set(ENV_AAUTH_MODE, "shadow");
    env.set(ENV_AAUTH_JWKS_PATH, "/tmp/does-not-matter");
    env.set(ENV_AAUTH_PERSON_SERVER_AUD, "https://ps.test");
    env.set(ENV_AAUTH_CHALLENGE_KID, "gw-kid");
    env.set(ENV_AAUTH_CHALLENGE_KEY_PATH, "/tmp/does-not-matter");
    assert!(matches!(
        gateway_aauth_from_env(&env),
        Err(AAuthEnvError::MissingRequired(ENV_AAUTH_RESOURCE_ISS))
    ));
}

#[test]
fn missing_person_server_aud_errors() {
    let env = InMemoryEnv::new();
    env.set(ENV_AAUTH_MODE, "shadow");
    env.set(ENV_AAUTH_JWKS_PATH, "/tmp/does-not-matter");
    env.set(ENV_AAUTH_RESOURCE_ISS, "https://resource.test");
    env.set(ENV_AAUTH_CHALLENGE_KID, "gw-kid");
    env.set(ENV_AAUTH_CHALLENGE_KEY_PATH, "/tmp/does-not-matter");
    assert!(matches!(
        gateway_aauth_from_env(&env),
        Err(AAuthEnvError::MissingRequired(ENV_AAUTH_PERSON_SERVER_AUD))
    ));
}

#[test]
fn missing_challenge_kid_errors() {
    let env = InMemoryEnv::new();
    env.set(ENV_AAUTH_MODE, "shadow");
    env.set(ENV_AAUTH_JWKS_PATH, "/tmp/does-not-matter");
    env.set(ENV_AAUTH_RESOURCE_ISS, "https://resource.test");
    env.set(ENV_AAUTH_PERSON_SERVER_AUD, "https://ps.test");
    env.set(ENV_AAUTH_CHALLENGE_KEY_PATH, "/tmp/does-not-matter");
    assert!(matches!(
        gateway_aauth_from_env(&env),
        Err(AAuthEnvError::MissingRequired(ENV_AAUTH_CHALLENGE_KID))
    ));
}

#[test]
fn missing_challenge_key_path_errors() {
    let env = InMemoryEnv::new();
    env.set(ENV_AAUTH_MODE, "shadow");
    env.set(ENV_AAUTH_JWKS_PATH, "/tmp/does-not-matter");
    env.set(ENV_AAUTH_RESOURCE_ISS, "https://resource.test");
    env.set(ENV_AAUTH_PERSON_SERVER_AUD, "https://ps.test");
    env.set(ENV_AAUTH_CHALLENGE_KID, "gw-kid");
    assert!(matches!(
        gateway_aauth_from_env(&env),
        Err(AAuthEnvError::MissingRequired(ENV_AAUTH_CHALLENGE_KEY_PATH))
    ));
}

#[test]
fn happy_path_shadow_builds_some() {
    let jwks_file = write_temp_file(r#"{"https://ap.test": {"keys": []}}"#);
    let key_file = write_temp_file(&ec_pem());
    let env = InMemoryEnv::new();
    env.set(ENV_AAUTH_MODE, "shadow");
    set_all_required(
        &env,
        jwks_file.path().to_str().expect("utf8 path"),
        key_file.path().to_str().expect("utf8 path"),
    );
    let result = gateway_aauth_from_env(&env).expect("shadow builds");
    assert!(result.is_some());
}

#[test]
fn happy_path_enforce_builds_some() {
    let jwks_file = write_temp_file(r#"{"https://ap.test": {"keys": []}}"#);
    let key_file = write_temp_file(&ec_pem());
    let env = InMemoryEnv::new();
    env.set(ENV_AAUTH_MODE, "enforce");
    set_all_required(
        &env,
        jwks_file.path().to_str().expect("utf8 path"),
        key_file.path().to_str().expect("utf8 path"),
    );
    let result = gateway_aauth_from_env(&env).expect("enforce builds");
    assert!(result.is_some());
}

fn set_all_required_without_jwks_path(env: &InMemoryEnv, challenge_key_path: &str) {
    env.set(ENV_AAUTH_RESOURCE_ISS, "https://resource.test");
    env.set(ENV_AAUTH_PERSON_SERVER_AUD, "https://ps.test");
    env.set(ENV_AAUTH_CHALLENGE_KID, "gw-kid");
    env.set(ENV_AAUTH_CHALLENGE_KEY_PATH, challenge_key_path);
}

#[test]
fn discovery_happy_path_builds_some() {
    let key_file = write_temp_file(&ec_pem());
    let env = InMemoryEnv::new();
    env.set(ENV_AAUTH_MODE, "shadow");
    set_all_required_without_jwks_path(&env, key_file.path().to_str().expect("utf8 path"));
    env.set(ENV_AAUTH_JWKS_DISCOVERY, "true");
    let result = gateway_aauth_from_env(&env).expect("discovery builds");
    assert!(result.is_some());
}

#[test]
fn discovery_false_with_no_path_errors_missing_source() {
    let key_file = write_temp_file(&ec_pem());
    let env = InMemoryEnv::new();
    env.set(ENV_AAUTH_MODE, "shadow");
    set_all_required_without_jwks_path(&env, key_file.path().to_str().expect("utf8 path"));
    env.set(ENV_AAUTH_JWKS_DISCOVERY, "false");
    assert!(matches!(
        gateway_aauth_from_env(&env),
        Err(AAuthEnvError::MissingJwksSource)
    ));
}

#[test]
fn both_path_and_discovery_set_errors_conflicting_source() {
    let jwks_file = write_temp_file(r#"{"https://ap.test": {"keys": []}}"#);
    let key_file = write_temp_file(&ec_pem());
    let env = InMemoryEnv::new();
    env.set(ENV_AAUTH_MODE, "shadow");
    set_all_required(
        &env,
        jwks_file.path().to_str().expect("utf8 path"),
        key_file.path().to_str().expect("utf8 path"),
    );
    env.set(ENV_AAUTH_JWKS_DISCOVERY, "true");
    assert!(matches!(
        gateway_aauth_from_env(&env),
        Err(AAuthEnvError::ConflictingJwksSource)
    ));
}

#[test]
fn invalid_discovery_flag_errors() {
    let key_file = write_temp_file(&ec_pem());
    let env = InMemoryEnv::new();
    env.set(ENV_AAUTH_MODE, "shadow");
    set_all_required_without_jwks_path(&env, key_file.path().to_str().expect("utf8 path"));
    env.set(ENV_AAUTH_JWKS_DISCOVERY, "bogus");
    assert!(matches!(
        gateway_aauth_from_env(&env),
        Err(AAuthEnvError::InvalidJwksDiscovery(_))
    ));
}

#[test]
fn discovery_with_custom_ttl_builds_some() {
    let key_file = write_temp_file(&ec_pem());
    let env = InMemoryEnv::new();
    env.set(ENV_AAUTH_MODE, "enforce");
    set_all_required_without_jwks_path(&env, key_file.path().to_str().expect("utf8 path"));
    env.set(ENV_AAUTH_JWKS_DISCOVERY, "true");
    env.set(ENV_AAUTH_JWKS_TTL_SECS, "120");
    let result = gateway_aauth_from_env(&env).expect("discovery with custom ttl builds");
    assert!(result.is_some());
}

#[test]
fn discovery_with_non_numeric_ttl_errors() {
    let key_file = write_temp_file(&ec_pem());
    let env = InMemoryEnv::new();
    env.set(ENV_AAUTH_MODE, "enforce");
    set_all_required_without_jwks_path(&env, key_file.path().to_str().expect("utf8 path"));
    env.set(ENV_AAUTH_JWKS_DISCOVERY, "true");
    env.set(ENV_AAUTH_JWKS_TTL_SECS, "not-a-number");
    assert!(matches!(
        gateway_aauth_from_env(&env),
        Err(AAuthEnvError::InvalidNonNegativeSecs {
            var: ENV_AAUTH_JWKS_TTL_SECS,
            ..
        })
    ));
}

#[test]
fn discovery_with_negative_ttl_errors() {
    let key_file = write_temp_file(&ec_pem());
    let env = InMemoryEnv::new();
    env.set(ENV_AAUTH_MODE, "enforce");
    set_all_required_without_jwks_path(&env, key_file.path().to_str().expect("utf8 path"));
    env.set(ENV_AAUTH_JWKS_DISCOVERY, "true");
    env.set(ENV_AAUTH_JWKS_TTL_SECS, "-1");
    assert!(matches!(
        gateway_aauth_from_env(&env),
        Err(AAuthEnvError::InvalidNonNegativeSecs {
            var: ENV_AAUTH_JWKS_TTL_SECS,
            ..
        })
    ));
}

#[test]
fn invalid_jwks_json_errors() {
    let jwks_file = write_temp_file("not json");
    let key_file = write_temp_file(&ec_pem());
    let env = InMemoryEnv::new();
    env.set(ENV_AAUTH_MODE, "enforce");
    set_all_required(
        &env,
        jwks_file.path().to_str().expect("utf8 path"),
        key_file.path().to_str().expect("utf8 path"),
    );
    assert!(matches!(
        gateway_aauth_from_env(&env),
        Err(AAuthEnvError::ParseJwks { .. })
    ));
}

#[test]
fn missing_jwks_file_errors() {
    let key_file = write_temp_file(&ec_pem());
    let env = InMemoryEnv::new();
    env.set(ENV_AAUTH_MODE, "enforce");
    set_all_required(
        &env,
        "/nonexistent/path/jwks.json",
        key_file.path().to_str().expect("utf8 path"),
    );
    assert!(matches!(
        gateway_aauth_from_env(&env),
        Err(AAuthEnvError::ReadFile { .. })
    ));
}

#[test]
fn invalid_challenge_key_pem_errors() {
    let jwks_file = write_temp_file(r#"{"https://ap.test": {"keys": []}}"#);
    let key_file = write_temp_file("not a pem");
    let env = InMemoryEnv::new();
    env.set(ENV_AAUTH_MODE, "enforce");
    set_all_required(
        &env,
        jwks_file.path().to_str().expect("utf8 path"),
        key_file.path().to_str().expect("utf8 path"),
    );
    assert!(matches!(
        gateway_aauth_from_env(&env),
        Err(AAuthEnvError::InvalidChallengeKey(_))
    ));
}

#[test]
fn defaults_apply_when_optional_vars_unset() {
    let jwks_file = write_temp_file(r#"{"https://ap.test": {"keys": []}}"#);
    let key_file = write_temp_file(&ec_pem());
    let env = InMemoryEnv::new();
    env.set(ENV_AAUTH_MODE, "enforce");
    set_all_required(
        &env,
        jwks_file.path().to_str().expect("utf8 path"),
        key_file.path().to_str().expect("utf8 path"),
    );
    let result = gateway_aauth_from_env(&env).expect("defaults apply");
    assert!(result.is_some());
}

#[test]
fn negative_or_non_numeric_optional_secs_errors() {
    let jwks_file = write_temp_file(r#"{"https://ap.test": {"keys": []}}"#);
    let key_file = write_temp_file(&ec_pem());

    let env = InMemoryEnv::new();
    env.set(ENV_AAUTH_MODE, "enforce");
    set_all_required(
        &env,
        jwks_file.path().to_str().expect("utf8 path"),
        key_file.path().to_str().expect("utf8 path"),
    );
    env.set(ENV_AAUTH_LEEWAY_SECS, "not-a-number");
    assert!(matches!(
        gateway_aauth_from_env(&env),
        Err(AAuthEnvError::InvalidNonNegativeSecs {
            var: ENV_AAUTH_LEEWAY_SECS,
            ..
        })
    ));

    let env2 = InMemoryEnv::new();
    env2.set(ENV_AAUTH_MODE, "enforce");
    set_all_required(
        &env2,
        jwks_file.path().to_str().expect("utf8 path"),
        key_file.path().to_str().expect("utf8 path"),
    );
    env2.set(ENV_AAUTH_CHALLENGE_TTL_SECS, "-5");
    assert!(matches!(
        gateway_aauth_from_env(&env2),
        Err(AAuthEnvError::InvalidNonNegativeSecs {
            var: ENV_AAUTH_CHALLENGE_TTL_SECS,
            ..
        })
    ));

    let env3 = InMemoryEnv::new();
    env3.set(ENV_AAUTH_MODE, "enforce");
    set_all_required(
        &env3,
        jwks_file.path().to_str().expect("utf8 path"),
        key_file.path().to_str().expect("utf8 path"),
    );
    env3.set(ENV_AAUTH_MAX_SKEW_SECS, "-1");
    assert!(matches!(
        gateway_aauth_from_env(&env3),
        Err(AAuthEnvError::InvalidNonNegativeSecs {
            var: ENV_AAUTH_MAX_SKEW_SECS,
            ..
        })
    ));
}

#[test]
fn aauth_deny_rule_fired_maps_all_variants() {
    let pop = AAuthDenyReason::Pop(NatsPopError::MissingHeader("x"));
    assert_eq!(aauth_deny_rule_fired(&pop), "gateway.aauth.denied.pop");

    let auth = AAuthDenyReason::Auth(TokenError::BadHeader);
    assert_eq!(aauth_deny_rule_fired(&auth), "gateway.aauth.denied.auth");

    let mismatch = AAuthDenyReason::AuthAgentMismatch {
        agent_sub: "agent-A".into(),
        agent_jkt: "jkt-A".into(),
        auth_agent: "agent-B".into(),
        auth_agent_jkt: "jkt-B".into(),
    };
    assert_eq!(
        aauth_deny_rule_fired(&mismatch),
        "gateway.aauth.denied.auth_agent_mismatch"
    );

    let scope = AAuthDenyReason::ScopeNotCovered {
        scope: "tasks.*".into(),
        method: "message.send".into(),
    };
    assert_eq!(aauth_deny_rule_fired(&scope), "gateway.aauth.denied.scope");

    let header = trogon_identity_types::aauth::MissionRef {
        approver: "approver-1".into(),
        s256: "hash-a".into(),
    };
    let claim = trogon_identity_types::aauth::MissionRef {
        approver: "approver-2".into(),
        s256: "hash-a".into(),
    };
    let mission_err =
        trogon_aauth_verify::mission::verify_mission_header_matches_claim(&header, &claim).expect_err("mismatch");
    let mission = AAuthDenyReason::MissionMismatch(mission_err);
    assert_eq!(aauth_deny_rule_fired(&mission), "gateway.aauth.denied.mission");

    let missing_header = AAuthDenyReason::MissionHeaderMissing {
        approver: "approver-1".into(),
    };
    assert_eq!(
        aauth_deny_rule_fired(&missing_header),
        "gateway.aauth.denied.mission_header_missing"
    );
}

fn jwt_header_identity() -> GatewayCallerIdentity {
    GatewayCallerIdentity {
        audit_caller_id: "user/jwt-header-caller".to_owned(),
        audit_caller_source: Some("jwt_header".to_owned()),
        caller_slug: Some("jwt-header-caller".to_owned()),
    }
}

#[test]
fn principal_present_overrides_existing_caller_identity() {
    let result = gateway_caller_identity_after_aauth(jwt_header_identity(), Some("alice"));
    assert_eq!(result.audit_caller_id, "user/alice");
    assert_eq!(result.caller_slug.as_deref(), Some("alice"));
}

#[test]
fn principal_present_sets_audit_caller_source_to_aauth() {
    let result = gateway_caller_identity_after_aauth(jwt_header_identity(), Some("alice"));
    assert_eq!(result.audit_caller_source.as_deref(), Some(AAUTH_CALLER_SOURCE));
    assert_eq!(AAUTH_CALLER_SOURCE, "aauth");
}

#[test]
fn principal_absent_preserves_existing_caller_identity() {
    let existing = jwt_header_identity();
    let result = gateway_caller_identity_after_aauth(existing.clone(), None);
    assert_eq!(result, existing);
}

#[test]
fn principal_present_overrides_even_anonymous_existing_identity() {
    let anonymous = GatewayCallerIdentity {
        audit_caller_id: "_".to_owned(),
        audit_caller_source: None,
        caller_slug: None,
    };
    let result = gateway_caller_identity_after_aauth(anonymous, Some("bob"));
    assert_eq!(result.audit_caller_id, "user/bob");
    assert_eq!(result.audit_caller_source.as_deref(), Some("aauth"));
    assert_eq!(result.caller_slug.as_deref(), Some("bob"));
}
