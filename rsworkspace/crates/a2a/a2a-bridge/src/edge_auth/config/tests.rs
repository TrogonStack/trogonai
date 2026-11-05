use trogon_aauth_verify::InMemoryReplayStore;
use trogon_std::env::InMemoryEnv;
use trogon_std::fs::MemFs;

use super::*;
use crate::edge_auth::{EdgeAdmission, EdgeAuthorityError, EdgeGuard};
use crate::edge_signing_harness::{
    CHALLENGE_KEY_PATH, EDGE_AUTHORITY, EDGE_RESOURCE, EdgeEnv, JWKS_PATH, Signing, SigningAgent,
};
use crate::error::EdgeAuthError;

fn config(edge: &EdgeEnv) -> Result<EdgeAuthConfig, EdgeAuthConfigError> {
    EdgeAuthConfig::from_env(edge.env(), edge.fs())
}

fn settings(edge: &EdgeEnv) -> ProofSettings {
    match config(edge).unwrap() {
        EdgeAuthConfig::Proof(settings) => settings,
        EdgeAuthConfig::Off => panic!("expected settings that verify proofs"),
    }
}

fn refusal(edge: &EdgeEnv) -> EdgeAuthConfigError {
    config(edge).unwrap_err()
}

fn trusting() -> EdgeEnv {
    EdgeEnv::trusting(&SigningAgent::new())
}

#[test]
fn enforce_is_the_mode_when_none_is_named() {
    let settings = settings(&trusting());
    assert_eq!(settings.policy(), ProofPolicy::Enforce);
    assert_eq!(settings.resource().as_str(), EDGE_RESOURCE);
    assert_eq!(settings.authority().as_str(), EDGE_AUTHORITY);
}

#[test]
fn signatures_are_held_to_one_minute_and_thirty_seconds_of_skew_by_default() {
    let settings = settings(&trusting());
    assert_eq!(settings.freshness().validity().secs(), 60);
    assert_eq!(settings.freshness().skew().secs(), 30);
    assert_eq!(settings.leeway().secs(), 30);
    assert!(settings.nonces().is_none());
    let rendered = format!("{settings:?}");
    assert!(rendered.contains(EDGE_RESOURCE));
    assert!(rendered.contains(EDGE_AUTHORITY));
    assert!(rendered.contains("Enforce"));
}

#[test]
fn a_mode_below_enforce_needs_the_dev_flag_as_well() {
    for (raw, mode) in [("shadow", EdgeAuthMode::Shadow), ("off", EdgeAuthMode::Off)] {
        let error = refusal(&trusting().set(ENV_AAUTH_MODE, raw));
        assert!(
            matches!(error, EdgeAuthConfigError::UnenforcedWithoutDevFlag(refused) if refused == mode),
            "{raw}: got {error:?}"
        );
    }
}

#[test]
fn the_dev_flag_grants_shadow() {
    let edge = trusting()
        .set(ENV_AAUTH_MODE, "shadow")
        .set(ENV_AAUTH_DEV_UNENFORCED, "true");
    assert_eq!(settings(&edge).policy(), ProofPolicy::Shadow);
}

#[test]
fn the_dev_flag_grants_off_without_any_proof_settings() {
    let env = InMemoryEnv::new();
    env.set(ENV_AAUTH_MODE, "off");
    env.set(ENV_AAUTH_DEV_UNENFORCED, "1");
    let config = EdgeAuthConfig::from_env(&env, &MemFs::new()).unwrap();
    assert!(matches!(config, EdgeAuthConfig::Off), "got {config:?}");
    assert_eq!(config.mode(), EdgeAuthMode::Off);
}

#[test]
fn the_dev_flag_alone_does_not_lower_enforce() {
    let edge = trusting().set(ENV_AAUTH_DEV_UNENFORCED, "yes");
    assert_eq!(settings(&edge).policy(), ProofPolicy::Enforce);
}

#[test]
fn modes_and_the_dev_flag_read_without_case_or_surrounding_space() {
    let edge = trusting()
        .set(ENV_AAUTH_MODE, " Shadow\n")
        .set(ENV_AAUTH_DEV_UNENFORCED, "YES");
    assert_eq!(config(&edge).unwrap().mode(), EdgeAuthMode::Shadow);
}

#[test]
fn an_unknown_mode_or_dev_flag_is_refused() {
    let error = refusal(&trusting().set(ENV_AAUTH_MODE, "permissive"));
    assert!(
        matches!(&error, EdgeAuthConfigError::InvalidMode(raw) if raw == "permissive"),
        "got {error:?}"
    );

    let error = refusal(
        &trusting()
            .set(ENV_AAUTH_MODE, "shadow")
            .set(ENV_AAUTH_DEV_UNENFORCED, "maybe"),
    );
    assert!(matches!(error, EdgeAuthConfigError::InvalidDevFlag(_)), "got {error:?}");
}

#[test]
fn the_resource_and_authority_have_to_be_named() {
    for var in [ENV_AAUTH_RESOURCE_ISS, ENV_AAUTH_AUTHORITY] {
        let error = refusal(&trusting().unset(var));
        assert!(
            matches!(error, EdgeAuthConfigError::MissingRequired(missing) if missing == var),
            "{var}: got {error:?}"
        );
    }
}

#[test]
fn an_authority_with_a_scheme_is_refused() {
    let error = refusal(&trusting().set(ENV_AAUTH_AUTHORITY, "https://bridge.test"));
    assert!(
        matches!(
            error,
            EdgeAuthConfigError::InvalidAuthority(EdgeAuthorityError::NotAnAuthority(_))
        ),
        "got {error:?}"
    );
}

#[test]
fn exactly_one_source_of_issuer_keys_is_accepted() {
    let error = refusal(&trusting().unset(ENV_AAUTH_JWKS_PATH));
    assert!(matches!(error, EdgeAuthConfigError::MissingJwksSource), "got {error:?}");

    let error = refusal(&trusting().set(ENV_AAUTH_JWKS_DISCOVERY, "true"));
    assert!(
        matches!(error, EdgeAuthConfigError::ConflictingJwksSource),
        "got {error:?}"
    );

    let error = refusal(&trusting().set(ENV_AAUTH_JWKS_DISCOVERY, "sometimes"));
    assert!(
        matches!(error, EdgeAuthConfigError::InvalidJwksDiscovery(_)),
        "got {error:?}"
    );

    let discovered = trusting()
        .unset(ENV_AAUTH_JWKS_PATH)
        .set(ENV_AAUTH_JWKS_DISCOVERY, "true")
        .set(
            ENV_AAUTH_JWKS_ALLOWED_ISSUERS,
            "https://agents.test, https://other.test",
        )
        .set(ENV_AAUTH_JWKS_TTL_SECS, "120");
    assert_eq!(settings(&discovered).policy(), ProofPolicy::Enforce);

    let unrestricted_discovery = trusting()
        .unset(ENV_AAUTH_JWKS_PATH)
        .set(ENV_AAUTH_JWKS_DISCOVERY, "true");
    assert_eq!(settings(&unrestricted_discovery).policy(), ProofPolicy::Enforce);
}

#[test]
fn an_unreadable_key_sets_file_names_its_variable() {
    let edge = trusting().set(ENV_AAUTH_JWKS_PATH, "/etc/aauth/missing.json");
    let error = refusal(&edge);
    assert!(
        matches!(&error, EdgeAuthConfigError::ReadFile { var, .. } if *var == ENV_AAUTH_JWKS_PATH),
        "got {error:?}"
    );
}

#[test]
fn a_key_sets_file_that_is_not_issuer_key_sets_is_refused() {
    let error = refusal(&trusting().file(JWKS_PATH, "{\"keys\": []}"));
    assert!(matches!(error, EdgeAuthConfigError::ParseJwks { .. }), "got {error:?}");
}

#[test]
fn signature_validity_is_bounded_to_two_minutes() {
    for raw in ["0", "121"] {
        let error = refusal(&trusting().set(ENV_AAUTH_SIGNATURE_VALIDITY_SECS, raw));
        assert!(
            matches!(error, EdgeAuthConfigError::InvalidSignatureValidity(_)),
            "{raw}: got {error:?}"
        );
    }
}

#[test]
fn clock_skew_and_token_leeway_are_bounded_to_thirty_seconds() {
    for var in [ENV_AAUTH_MAX_SKEW_SECS, ENV_AAUTH_LEEWAY_SECS] {
        let error = refusal(&trusting().set(var, "31"));
        assert!(
            matches!(&error, EdgeAuthConfigError::InvalidClockTolerance { var: refused, .. } if *refused == var),
            "{var}: got {error:?}"
        );
    }
}

#[test]
fn durations_have_to_be_whole_seconds() {
    let error = refusal(&trusting().set(ENV_AAUTH_SIGNATURE_VALIDITY_SECS, "1m"));
    assert!(
        matches!(&error, EdgeAuthConfigError::InvalidSecs { var, raw, .. }
            if *var == ENV_AAUTH_SIGNATURE_VALIDITY_SECS && raw == "1m"),
        "got {error:?}"
    );
}

#[test]
fn durations_within_their_bounds_are_kept_and_leeway_follows_skew() {
    let edge = trusting()
        .set(ENV_AAUTH_SIGNATURE_VALIDITY_SECS, "120")
        .set(ENV_AAUTH_MAX_SKEW_SECS, "10");
    let kept = settings(&edge);
    assert_eq!(kept.freshness().validity().secs(), 120);
    assert_eq!(kept.freshness().skew().secs(), 10);
    assert_eq!(kept.leeway().secs(), 10);

    let edge = trusting()
        .set(ENV_AAUTH_MAX_SKEW_SECS, "10")
        .set(ENV_AAUTH_LEEWAY_SECS, "5");
    assert_eq!(settings(&edge).leeway().secs(), 5);
}

#[test]
fn required_nonces_are_issued_under_the_configured_challenge_key() {
    let settings = settings(&trusting().requiring_nonces());
    let nonces = settings.nonces().unwrap();
    assert_eq!(nonces.requirement(), NonceRequirement::Required);
    assert_eq!(nonces.ttl(), NonceTtl::default());
}

#[test]
fn optional_nonces_are_issued_once_a_challenge_key_is_configured() {
    let edge = trusting().requiring_nonces().set(ENV_AAUTH_NONCE, "optional");
    let settings = settings(&edge);
    assert_eq!(settings.nonces().unwrap().requirement(), NonceRequirement::Optional);
}

#[test]
fn required_nonces_need_a_challenge_key_and_its_kid() {
    let error = refusal(&trusting().set(ENV_AAUTH_NONCE, "required"));
    assert!(
        matches!(error, EdgeAuthConfigError::MissingRequired(var) if var == ENV_AAUTH_CHALLENGE_KEY_PATH),
        "got {error:?}"
    );

    let error = refusal(&trusting().requiring_nonces().unset(ENV_AAUTH_CHALLENGE_KID));
    assert!(
        matches!(error, EdgeAuthConfigError::MissingRequired(var) if var == ENV_AAUTH_CHALLENGE_KID),
        "got {error:?}"
    );
}

#[test]
fn an_unknown_nonce_requirement_is_refused() {
    let error = refusal(&trusting().set(ENV_AAUTH_NONCE, "sometimes"));
    assert!(
        matches!(error, EdgeAuthConfigError::InvalidNonceRequirement(_)),
        "got {error:?}"
    );
}

#[test]
fn a_challenge_key_that_is_not_a_p256_key_is_refused() {
    let edge = trusting().requiring_nonces().file(CHALLENGE_KEY_PATH, "not a key");
    let error = refusal(&edge);
    assert!(
        matches!(error, EdgeAuthConfigError::InvalidChallengeKey(_)),
        "got {error:?}"
    );
}

#[test]
fn a_nonce_lifetime_beyond_two_minutes_is_refused() {
    let edge = trusting().requiring_nonces().set(ENV_AAUTH_NONCE_TTL_SECS, "121");
    let error = refusal(&edge);
    assert!(
        matches!(error, EdgeAuthConfigError::InvalidNonceTtl(_)),
        "got {error:?}"
    );
}

#[tokio::test]
async fn a_guard_built_from_the_environment_verifies_the_agent_it_trusts() {
    let agent = SigningAgent::new();
    let guard = settings(&EdgeEnv::trusting(&agent)).guard(Arc::new(InMemoryReplayStore::default()));
    assert_eq!(guard.mode(), EdgeAuthMode::Enforce);

    let admission = guard
        .admit(&agent.signed(b"{}", &Signing::fresh()), &agent.bearer())
        .await
        .unwrap();
    assert!(matches!(admission, EdgeAdmission::Verified(_)), "got {admission:?}");

    let stranger = SigningAgent::new();
    let error = guard
        .admit(&stranger.signed(b"{}", &Signing::fresh()), &stranger.bearer())
        .await
        .unwrap_err();
    assert!(matches!(error, EdgeAuthError::Unverified(_)), "got {error:?}");
}

#[tokio::test]
async fn a_guard_built_from_the_environment_demands_the_nonces_it_is_told_to() {
    let agent = SigningAgent::new();
    let guard = settings(&EdgeEnv::trusting(&agent).requiring_nonces()).guard(Arc::new(InMemoryReplayStore::default()));
    let error = guard
        .admit(&agent.signed(b"{}", &Signing::fresh()), &agent.bearer())
        .await
        .unwrap_err();
    assert!(matches!(error, EdgeAuthError::NonceRequired(_)), "got {error:?}");
}

#[tokio::test]
async fn a_guard_built_from_the_environment_holds_signatures_to_the_configured_validity() {
    let agent = SigningAgent::new();
    let edge = EdgeEnv::trusting(&agent).set(ENV_AAUTH_SIGNATURE_VALIDITY_SECS, "30");
    let guard = settings(&edge).guard(Arc::new(InMemoryReplayStore::default()));
    let error = guard
        .admit(&agent.signed(b"{}", &Signing::fresh()), &agent.bearer())
        .await
        .unwrap_err();
    assert!(matches!(error, EdgeAuthError::Unverified(_)), "got {error:?}");
}
