use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use trogon_sandbox_contract::DomainPattern;
use trogon_sandbox_contract::Extract;
use trogon_sandbox_contract::{ClaimName, JwtMasking};
use trogon_sandbox_contract::{
    CredentialProvider, CredentialRequest, CredentialScope, EgressViolationReason, PullSecretId,
};
use trogon_sandbox_contract::{
    EnvVarName, ExtractNoMatch, InjectHosts, MaskDuplicates, ResponseHeaderStrip, SecretBinding, SecretBindingParts,
    SecretBindings, SecretMode, SecretName, SecretTarget,
};
use trogon_sandbox_protocol::SentinelRegistry;

use crate::credential_source::tests::FakeCredentialSource;
use crate::credential_source::{SecretLoadError, load_sentinel_registry};
use crate::http_intercept::{RequestGuard, TlsTarget};
use crate::secret_substitution::{SecretSubstitution, SubstitutionScope};
use crate::substitution_tests::{body_of, exchange, text};

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

const HOST: &str = "api.example.com";
const SIGNATURE: &str = "cmVhbC1zaWduYXR1cmUtYnl0ZXM";

fn b64(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(value)
}

fn jwt(payload: &str) -> String {
    format!("{}.{}.{SIGNATURE}", b64(r#"{"alg":"RS256","typ":"JWT"}"#), b64(payload))
}

fn binding(claims: &[&str]) -> TestResult<SecretBinding> {
    let mut mask_claims = Vec::new();
    for claim in claims {
        mask_claims.push(ClaimName::new(*claim)?);
    }
    Ok(SecretBinding::new(SecretBindingParts {
        name: SecretName::new("tok")?,
        target: SecretTarget::Env(EnvVarName::new("TOK")?),
        mode: SecretMode::Mask,
        extract: Some(Extract::Jwt(JwtMasking {
            mask_claims,
            pattern: None,
        })),
        mask_duplicates: MaskDuplicates::Disabled,
        on_extract_no_match: ExtractNoMatch::default(),
        strip_response_headers: ResponseHeaderStrip::default(),
        inject_hosts: InjectHosts::new(vec![DomainPattern::new(HOST)?])?,
        inject_scope: Default::default(),
        source: CredentialRequest {
            provider: CredentialProvider::Registry,
            pull_secret_id: PullSecretId::new("one"),
            scope: CredentialScope::RegistryPull {
                repository_glob: "*".to_string(),
            },
            ttl_seconds: 60,
            requester: None,
        },
    })?)
}

async fn load(real: &str, claims: &[&str]) -> TestResult<Result<Arc<SentinelRegistry>, SecretLoadError>> {
    let source = FakeCredentialSource::new().with_token("one", real);
    let bindings = SecretBindings::new(vec![binding(claims)?])?;
    Ok(load_sentinel_registry(&bindings, &source).await.map(Arc::new))
}

fn scope(registry: &Arc<SentinelRegistry>, host: &str) -> TestResult<SubstitutionScope> {
    Ok(SecretSubstitution::new(registry.clone())
        .scope_for(&TlsTarget::new(host, 443))
        .ok_or("expected a scope")?)
}

fn rewrite(scope: &SubstitutionScope, input: &str) -> TestResult<String> {
    let mut scanner = scope.scanner();
    let mut out = Vec::new();
    scanner.feed(input.as_bytes(), &mut out);
    scanner.finish(&mut out);
    Ok(String::from_utf8(out)?)
}

fn scrub(scope: &SubstitutionScope, input: &str) -> TestResult<String> {
    let mut scanner = scope.response_rules().ok_or("expected response rules")?.scanner();
    let mut out = Vec::new();
    scanner.feed(input.as_bytes(), &mut out);
    scanner.finish(&mut out);
    Ok(String::from_utf8(out)?)
}

fn sentinel(registry: &SentinelRegistry, name: &str) -> TestResult<String> {
    Ok(registry
        .sentinel_for(&SecretName::new(name)?)
        .ok_or("expected a sentinel")?
        .to_string())
}

#[tokio::test]
async fn the_whole_fake_token_is_swapped_for_the_real_one_on_the_inject_host() -> TestResult<()> {
    let real = jwt(r#"{"sub":"alice-the-user","scope":"admin"}"#);
    let registry = load(&real, &[]).await??;
    let fake = sentinel(&registry, "tok")?;
    assert_ne!(fake, real);
    assert!(!fake.contains(SIGNATURE));

    let out = rewrite(&scope(&registry, HOST)?, &format!("Authorization: Bearer {fake}\r\n"))?;

    assert_eq!(out, format!("Authorization: Bearer {real}\r\n"));
    Ok(())
}

#[tokio::test]
async fn a_claim_sent_alone_is_swapped_and_so_is_the_rebuilt_token() -> TestResult<()> {
    let real = jwt(r#"{"sub":"alice-the-user","iss":"idp"}"#);
    let registry = load(&real, &["sub"]).await??;
    let fake = sentinel(&registry, "tok")?;
    let claim = sentinel(&registry, "tok.claim-0")?;
    let scope = scope(&registry, HOST)?;

    assert_eq!(rewrite(&scope, &fake)?, real);
    assert_eq!(rewrite(&scope, &claim)?, "alice-the-user");
    assert!(!fake.contains("alice-the-user"));
    Ok(())
}

#[tokio::test]
async fn neither_the_token_nor_its_claims_are_swapped_off_host() -> TestResult<()> {
    let real = jwt(r#"{"sub":"alice-the-user"}"#);
    let registry = load(&real, &["sub"]).await??;
    let fake = sentinel(&registry, "tok")?;
    let claim = sentinel(&registry, "tok.claim-0")?;

    let out = rewrite(&scope(&registry, "other.example.net")?, &format!("{fake} {claim}"))?;

    assert_eq!(out, format!("{fake} {claim}"));
    Ok(())
}

#[tokio::test]
async fn a_reflected_real_token_and_claim_are_scrubbed_from_responses() -> TestResult<()> {
    let real = jwt(r#"{"sub":"alice-the-user"}"#);
    let registry = load(&real, &["sub"]).await??;
    let fake = sentinel(&registry, "tok")?;
    let claim = sentinel(&registry, "tok.claim-0")?;

    let out = scrub(&scope(&registry, HOST)?, &format!("echo {real} and alice-the-user"))?;

    assert_eq!(out, format!("echo {fake} and {claim}"));
    assert!(!out.contains(SIGNATURE));
    Ok(())
}

#[tokio::test]
async fn a_claim_below_the_scrub_threshold_is_swapped_but_never_scrubbed() -> TestResult<()> {
    let real = jwt(r#"{"sub":"bob","org":"acme"}"#);
    let registry = load(&real, &["sub", "org"]).await??;
    let claim = sentinel(&registry, "tok.claim-0")?;
    let scope = scope(&registry, HOST)?;

    assert_eq!(rewrite(&scope, &claim)?, "bob");
    assert_eq!(scrub(&scope, "bob is not a secret here")?, "bob is not a secret here");
    Ok(())
}

#[tokio::test]
async fn the_registry_survives_the_handoff_with_jwt_shaped_sentinels() -> TestResult<()> {
    let real = jwt(r#"{"sub":"alice-the-user"}"#);
    let registry = load(&real, &["sub"]).await??;
    let fake = sentinel(&registry, "tok")?;

    let restored = Arc::new(trogon_sandbox_protocol::RegistryHandoff::from_registry(&registry)?.into_registry()?);

    assert_eq!(sentinel(&restored, "tok")?, fake);
    assert_eq!(rewrite(&scope(&restored, HOST)?, &fake)?, real);
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "the display never quoting the secret is the contract under test"
    )
)]
async fn a_value_that_is_not_a_jwt_is_refused_with_a_typed_reason() -> TestResult<()> {
    for value in [
        "ghp_plainapitoken0123456789",
        "a.b.c",
        &format!("{}.{}.sig", b64("{}"), b64(r#"{"a":"b"}"#)),
        &format!("{}.{}.sig", b64(r#"{"alg":"HS256"}"#), b64("not json")),
    ] {
        let err = load(value, &[]).await?.err().ok_or("expected a value")?;
        assert!(matches!(err, SecretLoadError::Jwt { .. }), "{value}");
        assert_eq!(err.violation_reason(), Some(EgressViolationReason::SecretJwtMalformed));
        assert!(!err.to_string().contains(value) || value.len() < 6);
    }
    Ok(())
}

#[tokio::test]
async fn naming_claims_the_token_lacks_is_refused_with_a_typed_reason() -> TestResult<()> {
    let real = jwt(r#"{"sub":"alice-the-user"}"#);
    let err = load(&real, &["email"]).await?.err().ok_or("expected a value")?;
    assert_eq!(
        err.violation_reason(),
        Some(EgressViolationReason::SecretJwtClaimsUnmatched)
    );
    Ok(())
}

#[tokio::test]
async fn a_jwt_shaped_sentinel_in_a_body_takes_the_re_chunked_path_because_its_length_differs() -> TestResult<()> {
    let real = jwt(r#"{"sub":"alice-the-user","scope":"admin"}"#);
    let registry = load(&real, &[]).await??;
    let fake = sentinel(&registry, "tok")?;
    let scope = scope(&registry, HOST)?;
    assert_ne!(fake.len(), real.len());
    assert!(!scope.keeps_body_length());
    let body = format!("{{\"token\":\"{fake}\"}}");
    let request = format!(
        "POST /v1 HTTP/1.1\r\nHost: {HOST}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let guard = RequestGuard::new(TlsTarget::new(HOST, 443)).with_substitution(Some(scope));

    let outcome = exchange(guard, None, request.as_bytes()).await?;

    let received = text(&outcome.upstream);
    assert!(!received.to_ascii_lowercase().contains("content-length"));
    assert!(received.contains("Transfer-Encoding: chunked\r\n"));
    assert_eq!(body_of(&outcome.upstream)?, format!("{{\"token\":\"{real}\"}}"));
    Ok(())
}
