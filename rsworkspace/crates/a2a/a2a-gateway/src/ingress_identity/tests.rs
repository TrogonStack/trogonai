use std::time::{Duration, SystemTime};

use a2a_auth_callout::jwt::ExternalSubject;
use a2a_auth_callout::{
    CallerId, IssuedPermissions, KeyVersion, NkeyPublic, SpiceDbPrincipal, StaticSigningKeySource, UserJwtSubject,
};
use nkeys::KeyPair;
use trogon_identity_types::PrincipalKind;

use super::*;

struct Issuer {
    source: Arc<StaticSigningKeySource>,
}

impl Issuer {
    fn new() -> Self {
        let seed = KeyPair::new_account().seed().expect("issuer seed");
        let source = StaticSigningKeySource::new(seed.as_str(), KeyVersion::new("current").expect("version"))
            .expect("static source");
        Self {
            source: Arc::new(source),
        }
    }

    fn verifier(&self) -> ForwardedCallerJwtVerifier {
        ForwardedCallerJwtVerifier::new(self.source.clone(), AccountName::new("tenant-acme"))
    }

    fn mint(&self, kind: PrincipalKind, caller: &str) -> String {
        self.mint_with(kind, caller, "tenant-acme", SystemTime::now())
    }

    fn mint_with(&self, kind: PrincipalKind, caller: &str, audience: &str, issued_at: SystemTime) -> String {
        let caller_id = CallerId::new(caller).expect("caller");
        let claims = UserJwtClaims {
            kid: KeyVersion::new("current").expect("version"),
            sub: ExternalSubject::new(caller).expect("sub"),
            aud: AccountName::new(audience),
            data: SpiceDbPrincipal::new(format!("user/{caller}")),
            nats_permissions: IssuedPermissions::default_for_caller(&caller_id),
            caller_id,
            principal_kind: kind,
        };
        let user = KeyPair::new_user();
        let subject = UserJwtSubject::from_user_nkey(NkeyPublic::parse(user.public_key()).expect("user nkey"));
        let token = claims
            .mint(
                &self.source.current().minting_material(),
                &subject,
                issued_at,
                Duration::from_secs(300),
            )
            .expect("mint");
        CallerJwtHeaderValue::from_minted(&token).as_str().to_owned()
    }
}

fn alice() -> SubjectPrincipal {
    SubjectPrincipal::from_tokens("person", "alice").expect("principal")
}

fn headers_with(name: &str, value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(name, value);
    headers
}

#[test]
fn absent_caller_jwt_passes() {
    let issuer = Issuer::new();
    let mut headers = HeaderMap::new();
    headers.insert("X-Correlation", "abc");

    assert!(issuer.verifier().check(&HeaderMap::new(), &alice()).is_ok());
    assert!(issuer.verifier().check(&headers, &alice()).is_ok());
}

#[test]
fn caller_jwt_naming_the_subject_principal_passes() {
    let issuer = Issuer::new();
    let headers = headers_with(CALLER_JWT_HEADER_NAME, &issuer.mint(PrincipalKind::Person, "alice"));

    assert!(issuer.verifier().check(&headers, &alice()).is_ok());
}

#[test]
fn caller_jwt_naming_another_principal_is_rejected() {
    let issuer = Issuer::new();
    for (kind, id) in [(PrincipalKind::Person, "bob"), (PrincipalKind::Agent, "alice")] {
        let headers = headers_with(CALLER_JWT_HEADER_NAME, &issuer.mint(kind, id));

        let rejection = issuer.verifier().check(&headers, &alice()).unwrap_err();

        let ForwardedCallerJwtError::PrincipalMismatch { named, subject } = &rejection else {
            panic!("expected a principal mismatch, got {rejection:?}");
        };
        assert_eq!(named.to_string(), format!("{}.{id}", kind.as_str()));
        assert_eq!(subject, &alice());
        assert_eq!(rejection.audit_rule(), "gateway.caller_jwt.principal_mismatch");
    }
}

#[test]
fn caller_jwt_that_is_not_a_compact_jwt_is_rejected() {
    let issuer = Issuer::new();
    for garbage in ["", "   ", "not-a-jwt", "a.b", "a..c"] {
        let headers = headers_with(CALLER_JWT_HEADER_NAME, garbage);

        let rejection = issuer.verifier().check(&headers, &alice()).unwrap_err();

        assert!(
            matches!(rejection, ForwardedCallerJwtError::Malformed(_)),
            "{garbage:?} gave {rejection:?}"
        );
        assert_eq!(rejection.audit_rule(), "gateway.caller_jwt.malformed");
    }
}

#[test]
fn caller_jwt_for_another_audience_is_rejected() {
    let issuer = Issuer::new();
    let token = issuer.mint_with(PrincipalKind::Person, "alice", "tenant-other", SystemTime::now());
    let headers = headers_with(CALLER_JWT_HEADER_NAME, &token);

    let rejection = issuer.verifier().check(&headers, &alice()).unwrap_err();

    assert!(matches!(rejection, ForwardedCallerJwtError::Unverified(_)));
    assert_eq!(rejection.audit_rule(), "gateway.caller_jwt.unverified");
}

#[test]
fn expired_caller_jwt_is_rejected() {
    let issuer = Issuer::new();
    let issued_at = SystemTime::now() - Duration::from_secs(3_600);
    let token = issuer.mint_with(PrincipalKind::Person, "alice", "tenant-acme", issued_at);
    let headers = headers_with(CALLER_JWT_HEADER_NAME, &token);

    let rejection = issuer.verifier().check(&headers, &alice()).unwrap_err();

    assert!(matches!(rejection, ForwardedCallerJwtError::Unverified(_)));
}

#[test]
fn caller_jwt_from_another_issuer_is_rejected() {
    let gateway = Issuer::new();
    let stranger = Issuer::new();
    let headers = headers_with(CALLER_JWT_HEADER_NAME, &stranger.mint(PrincipalKind::Person, "alice"));

    let rejection = gateway.verifier().check(&headers, &alice()).unwrap_err();

    assert!(matches!(rejection, ForwardedCallerJwtError::Unverified(_)));
}

#[test]
fn caller_jwt_header_is_checked_whatever_its_case() {
    let issuer = Issuer::new();
    let headers = headers_with("a2a-caller-jwt", &issuer.mint(PrincipalKind::Person, "bob"));

    let rejection = issuer.verifier().check(&headers, &alice()).unwrap_err();

    assert!(matches!(rejection, ForwardedCallerJwtError::PrincipalMismatch { .. }));
}

#[test]
fn every_forwarded_caller_jwt_must_name_the_subject_principal() {
    let issuer = Issuer::new();
    let mut headers = headers_with(CALLER_JWT_HEADER_NAME, &issuer.mint(PrincipalKind::Person, "alice"));
    headers.append(
        CALLER_JWT_HEADER_NAME,
        issuer.mint(PrincipalKind::Person, "bob").as_str(),
    );

    let rejection = issuer.verifier().check(&headers, &alice()).unwrap_err();

    assert!(matches!(rejection, ForwardedCallerJwtError::PrincipalMismatch { .. }));
}

#[test]
fn identity_headers_are_stripped_whatever_their_case() {
    let mut headers = HeaderMap::new();
    headers.insert(CALLER_JWT_HEADER_NAME, "a.b.c");
    headers.insert("a2a-caller-jwt", "d.e.f");
    headers.insert(GATEWAY_CALLER_ID_HEADER, "mallory");
    headers.insert("x-a2a-caller-id", "mallory");
    headers.insert(GATEWAY_PRINCIPAL_HEADER, r#"{"spicedb_subject":"user/mallory"}"#);
    headers.insert("X-A2A-SPICEDB-PRINCIPAL", r#"{"spicedb_subject":"user/mallory"}"#);
    headers.insert("Trogon-Req-Id", "req-1");
    headers.append("X-Correlation", "first");
    headers.append("X-Correlation", "second");

    let kept = without_identity_headers(&headers);

    assert_eq!(kept.len(), 2);
    assert_eq!(kept.get("Trogon-Req-Id").map(HeaderValue::as_str), Some("req-1"));
    assert_eq!(
        kept.get_all("X-Correlation")
            .map(HeaderValue::as_str)
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
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
