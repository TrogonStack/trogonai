use std::sync::Arc;
use std::time::{Duration, SystemTime};

use a2a_auth_callout::{
    CallerId, IssuedPermissions, KeyVersion, StaticSigningKeySource, UserJwtSubject, jwt::ExternalSubject,
};
use a2a_nats::constants::{GATEWAY_CALLER_ID_HEADER, GATEWAY_PRINCIPAL_HEADER};
use async_nats::HeaderMap;
use nkeys::KeyPair;
use serde_json::json;

use super::*;

struct TestMessageIdentity {
    verified: Option<VerifiedCallerIdentity>,
}

impl MessageCallerIdentitySource for TestMessageIdentity {
    fn verified_caller_identity(&self, _message: &async_nats::Message) -> Option<VerifiedCallerIdentity> {
        self.verified.clone()
    }
}

fn principal_headers(subject: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        GATEWAY_PRINCIPAL_HEADER,
        format!(r#"{{"spicedb_subject":"{subject}","aud":"tenant-acme"}}"#),
    );
    headers
}

fn trust_headers_on() -> GatewayCallerIdentityPolicy {
    GatewayCallerIdentityPolicy {
        trust_caller_headers: TrustCallerHeaders(true),
    }
}

fn trust_headers_off() -> GatewayCallerIdentityPolicy {
    GatewayCallerIdentityPolicy::production_default()
}

fn empty_message() -> async_nats::Message {
    async_nats::Message {
        subject: "a2a.gateway.bot.message.send".into(),
        reply: Some("_INBOX.reply".into()),
        payload: vec![].into(),
        headers: None,
        status: None,
        description: None,
        length: 0,
    }
}

#[test]
fn message_verified_principal_wins_over_headers() {
    let principal = SpiceDbPrincipal::new("msg-user");
    let message_identity = TestMessageIdentity {
        verified: VerifiedCallerIdentity::from_principal(principal),
    };
    let headers = principal_headers("header-user");
    let msg = empty_message();

    let identity =
        resolve_gateway_caller_identity(&message_identity, &msg, &headers, trust_headers_on()).expect("identity");
    assert_eq!(identity.spicedb_subject.as_str(), "msg-user");
    assert_eq!(identity.source, CallerIdentitySource::MessageCallerJwtHeader);
}

#[test]
fn trust_headers_off_ignores_principal_and_caller_id_headers() {
    let message_identity = TestMessageIdentity { verified: None };
    let mut headers = principal_headers("spicedb-user-1");
    headers.insert(GATEWAY_CALLER_ID_HEADER, "legacy-caller");
    let msg = empty_message();

    assert!(resolve_gateway_caller_identity(&message_identity, &msg, &headers, trust_headers_off()).is_none());
}

#[test]
fn trust_headers_on_principal_header_uses_header_principal_source() {
    let message_identity = TestMessageIdentity { verified: None };
    let mut headers = principal_headers("spicedb-user-1");
    headers.insert(GATEWAY_CALLER_ID_HEADER, "legacy-caller");
    let msg = empty_message();

    let identity =
        resolve_gateway_caller_identity(&message_identity, &msg, &headers, trust_headers_on()).expect("identity");
    assert_eq!(identity.spicedb_subject.as_str(), "spicedb-user-1");
    assert_eq!(identity.source, CallerIdentitySource::HeaderPrincipal);
    assert_eq!(identity.audience.as_deref(), Some("tenant-acme"));

    let (caller_id, source) = gateway_audit_caller_attribution(Some(identity));
    assert_eq!(caller_id, "spicedb-user-1");
    assert_eq!(source.as_deref(), Some("header_principal"));
}

#[test]
fn trust_headers_on_caller_id_header_only_uses_header_caller_id_source() {
    let message_identity = TestMessageIdentity { verified: None };
    let mut headers = HeaderMap::new();
    headers.insert(GATEWAY_CALLER_ID_HEADER, "bridge-caller");
    let msg = empty_message();

    let identity =
        resolve_gateway_caller_identity(&message_identity, &msg, &headers, trust_headers_on()).expect("identity");
    assert_eq!(identity.spicedb_subject.as_str(), "bridge-caller");
    assert_eq!(identity.source, CallerIdentitySource::HeaderCallerId);

    let (caller_id, source) = gateway_audit_caller_attribution(Some(identity));
    assert_eq!(caller_id, "bridge-caller");
    assert_eq!(source.as_deref(), Some("header_trusted"));
}

#[test]
fn neither_message_nor_trusted_headers_yields_audit_fallback() {
    let message_identity = TestMessageIdentity { verified: None };
    let headers = HeaderMap::new();
    let msg = empty_message();
    assert!(resolve_gateway_caller_identity(&message_identity, &msg, &headers, trust_headers_off()).is_none());

    let (caller_id, source) = gateway_audit_caller_attribution(None);
    assert_eq!(caller_id, "_");
    assert!(source.is_none());
}

#[test]
fn gateway_jwt_audience_falls_back_to_prefix_when_unset() {
    struct EmptyEnv;
    impl ReadEnv for EmptyEnv {
        fn var(&self, _key: &str) -> Result<String, std::env::VarError> {
            Err(std::env::VarError::NotPresent)
        }
    }
    let aud = gateway_jwt_audience(&EmptyEnv, "fallback-account");
    assert_eq!(aud.as_str(), "fallback-account");
}

#[test]
fn gateway_jwt_audience_reads_env_value_trimmed() {
    struct WithAud;
    impl ReadEnv for WithAud {
        fn var(&self, key: &str) -> Result<String, std::env::VarError> {
            if key == ENV_GATEWAY_JWT_AUDIENCE {
                Ok("  configured-aud  ".into())
            } else {
                Err(std::env::VarError::NotPresent)
            }
        }
    }
    let aud = gateway_jwt_audience(&WithAud, "fallback-account");
    assert_eq!(aud.as_str(), "configured-aud");
}

#[test]
fn gateway_jwt_audience_ignores_whitespace_only_env_value() {
    struct OnlySpaces;
    impl ReadEnv for OnlySpaces {
        fn var(&self, key: &str) -> Result<String, std::env::VarError> {
            if key == ENV_GATEWAY_JWT_AUDIENCE {
                Ok("   ".into())
            } else {
                Err(std::env::VarError::NotPresent)
            }
        }
    }
    let aud = gateway_jwt_audience(&OnlySpaces, "fallback-account");
    assert_eq!(aud.as_str(), "fallback-account");
}

#[test]
fn verified_caller_identity_accessors_round_trip() {
    let aud = AudienceFromPrincipal("tenant-acme".into());
    assert_eq!(aud.as_str(), "tenant-acme");

    let principal = SpiceDbPrincipal::new("user-1");
    let identity = VerifiedCallerIdentity::new(principal, Some(aud)).expect("subject present");
    assert_eq!(identity.principal().spicedb_subject().unwrap().as_str(), "user-1");
    assert_eq!(
        identity.audience().map(AudienceFromPrincipal::as_str),
        Some("tenant-acme")
    );
}

#[test]
fn verified_caller_identity_new_rejects_principal_without_subject() {
    let principal = SpiceDbPrincipal(serde_json::json!({"not_subject": "x"}));
    assert!(VerifiedCallerIdentity::new(principal, None).is_none());
}

#[test]
fn principal_from_headers_rejects_missing_spicedb_subject() {
    let mut headers = HeaderMap::new();
    headers.insert(
        GATEWAY_PRINCIPAL_HEADER,
        r#"{"aud":"tenant-acme"}"#, // no spicedb_subject
    );
    let message_identity = TestMessageIdentity { verified: None };
    let msg = empty_message();
    assert!(resolve_gateway_caller_identity(&message_identity, &msg, &headers, trust_headers_on()).is_none());
}

#[test]
fn principal_from_headers_rejects_empty_spicedb_subject() {
    let mut headers = HeaderMap::new();
    headers.insert(GATEWAY_PRINCIPAL_HEADER, r#"{"spicedb_subject":"  "}"#);
    let message_identity = TestMessageIdentity { verified: None };
    let msg = empty_message();
    assert!(resolve_gateway_caller_identity(&message_identity, &msg, &headers, trust_headers_on()).is_none());
}

#[test]
fn caller_identity_source_audit_labels_cover_every_variant() {
    assert_eq!(CallerIdentitySource::MessageCallerJwtHeader.audit_label(), "jwt_header");
    assert_eq!(CallerIdentitySource::HeaderPrincipal.audit_label(), "header_principal");
    assert_eq!(CallerIdentitySource::HeaderCallerId.audit_label(), "header_trusted");
}

#[test]
fn trust_caller_headers_env_on_emits_warn_and_returns_enabled_policy() {
    struct WithTrust;
    impl ReadEnv for WithTrust {
        fn var(&self, key: &str) -> Result<String, std::env::VarError> {
            if key == ENV_GATEWAY_TRUST_CALLER_HEADERS {
                Ok("true".into())
            } else {
                Err(std::env::VarError::NotPresent)
            }
        }
    }
    // Exercises the `call_once` warn! path. The static Once guards against
    // log noise across the test binary; we only assert the policy result.
    let policy = gateway_caller_identity_policy(&WithTrust);
    assert!(policy.trust_caller_headers.is_enabled());
}

#[test]
fn trust_caller_headers_env_defaults_off() {
    struct EmptyEnv;
    impl ReadEnv for EmptyEnv {
        fn var(&self, _key: &str) -> Result<String, std::env::VarError> {
            Err(std::env::VarError::NotPresent)
        }
    }
    assert!(
        !gateway_caller_identity_policy(&EmptyEnv)
            .trust_caller_headers
            .is_enabled()
    );
}

#[test]
fn jwt_header_source_verifies_minted_token() {
    let issuer = KeyPair::new_account();
    let user = KeyPair::new_user();
    let issuer_seed = issuer.seed().expect("issuer seed");
    let source = Arc::new(
        StaticSigningKeySource::new(issuer_seed.as_str(), KeyVersion::new("current").expect("version"))
            .expect("static source"),
    );
    let caller_id = CallerId::new("caller1").expect("caller");
    let claims = UserJwtClaims {
        kid: KeyVersion::new("current").expect("version"),
        sub: ExternalSubject::new("alice").expect("sub"),
        aud: AccountName::new("tenant-acme"),
        data: SpiceDbPrincipal(json!({"spicedb_subject": "jwt-header-user"})),
        nats_permissions: IssuedPermissions::default_for_caller(&caller_id),
        caller_id,
    };
    let subject = UserJwtSubject::from_user_nkey(a2a_auth_callout::NkeyPublic::parse(user.public_key()).unwrap());
    let handle = source.current();
    let issued_at = SystemTime::now();
    let token = claims
        .mint(
            &handle.minting_material(),
            &subject,
            issued_at,
            Duration::from_secs(300),
        )
        .expect("mint");

    let mut headers = HeaderMap::new();
    headers.insert(
        CALLER_JWT_HEADER_NAME,
        CallerJwtHeaderValue::from_minted(&token).as_str(),
    );
    let mut msg = empty_message();
    msg.headers = Some(headers);

    let identity_source = JwtHeaderCallerIdentitySource::new(source, AccountName::new("tenant-acme"));
    let verified = identity_source.verified_caller_identity(&msg).expect("verified");
    assert_eq!(
        verified.principal().spicedb_subject().unwrap().as_str(),
        "jwt-header-user"
    );
}
