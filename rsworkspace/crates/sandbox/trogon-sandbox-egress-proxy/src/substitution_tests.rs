use std::collections::BTreeSet;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};
use tracing_subscriber::fmt::MakeWriter;
use trogon_sandbox_contract::DomainPattern;
use trogon_sandbox_contract::{
    CredentialInjection, CredentialProvider, CredentialRequest, CredentialScope, CredentialTransform, PullSecretId,
};
use trogon_sandbox_contract::{
    EnvVarName, ExtractNoMatch, HeaderName, InjectHosts, MaskDuplicates, ResponseHeaderStrip, SecretBinding,
    SecretBindingParts, SecretBindings, SecretMode, SecretName, SecretTarget,
};
use trogon_sandbox_protocol::{RealValue, SentinelRegistry};

use crate::credential_source::tests::FakeCredentialSource;
use crate::credential_source::{SecretLoadError, load_sentinel_registry};
use crate::http_intercept::{
    BodyLimit, CompiledMatchRules, Injection, RequestGuard, TlsTarget, run_compiled_http_intercept,
};
use crate::secret_substitution::tests::{API_REAL, Fixture, REGISTRY_REAL, fixture, scope as scope_for};
use crate::secret_substitution::{SecretSubstitution, SentinelMisdirectionObserver};
use crate::upstream_leg::UpstreamLeg;

pub(crate) type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

/// Locks `mutex`, reporting a poisoned lock as an error instead of panicking.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> TestResult<MutexGuard<'_, T>> {
    mutex.lock().map_err(|_| "poisoned lock".into())
}

fn hosts(host: &str) -> TestResult<InjectHosts> {
    Ok(InjectHosts::new(vec![DomainPattern::new(host)?])?)
}

const API_HOST: &str = "api.example.com";
const REGISTRY_HOST: &str = "registry.example.org";
const OK: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";

#[derive(Default)]
struct RecordingObserver {
    seen: Mutex<Vec<(String, u16, String)>>,
    compressed: Mutex<Vec<(String, u16, String)>>,
}

#[async_trait]
impl SentinelMisdirectionObserver for RecordingObserver {
    async fn sentinel_off_host(&self, host: &str, port: u16, binding: &SecretName) {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push((host.to_string(), port, binding.to_string()));
        }
    }

    async fn compressed_response(&self, host: &str, port: u16, binding: &SecretName) {
        if let Ok(mut compressed) = self.compressed.lock() {
            compressed.push((host.to_string(), port, binding.to_string()));
        }
    }
}

pub(crate) struct Outcome {
    pub(crate) upstream: Vec<u8>,
    pub(crate) client: Vec<u8>,
}

pub(crate) async fn exchange(
    guard: RequestGuard,
    injection: Option<(&CredentialTransform, &FakeCredentialSource)>,
    request: &[u8],
) -> TestResult<Outcome> {
    let (mut client_side, proxy_client) = duplex(256 * 1024);
    let (proxy_upstream, mut upstream_side) = duplex(256 * 1024);

    let upstream = tokio::spawn(async move {
        let mut seen = Vec::new();
        let mut chunk = vec![0u8; 128 * 1024];
        while !request_complete(&seen)? {
            let n = upstream_side.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            seen.extend_from_slice(&chunk[..n]);
        }
        if !seen.is_empty() {
            upstream_side.write_all(OK).await?;
        }
        Ok::<_, io::Error>(seen)
    });

    client_side.write_all(request).await?;
    client_side.shutdown().await?;

    let rules = CompiledMatchRules::compile(&[])?;
    let injection = injection.map(|(transform, source)| Injection {
        transform,
        match_rules: &rules,
        credential_source: source,
    });
    run_compiled_http_intercept(proxy_client, UpstreamLeg::connected(proxy_upstream), injection, &guard).await?;

    let mut client = Vec::new();
    client_side.read_to_end(&mut client).await?;
    Ok(Outcome {
        upstream: upstream.await??,
        client,
    })
}

fn request_complete(seen: &[u8]) -> io::Result<bool> {
    let Some(head_end) = seen.windows(4).position(|window| window == b"\r\n\r\n") else {
        return Ok(false);
    };
    let head = String::from_utf8_lossy(&seen[..head_end]).to_ascii_lowercase();
    let body = &seen[head_end + 4..];
    if let Some(length) = head.lines().find_map(|line| line.strip_prefix("content-length:")) {
        return Ok(body.len() >= length.trim().parse::<usize>().map_err(io::Error::other)?);
    }
    if head.contains("transfer-encoding: chunked") {
        return Ok(body.ends_with(b"0\r\n\r\n"));
    }
    Ok(true)
}

fn guard_for(fx: &Fixture, host: &str) -> TestResult<RequestGuard> {
    Ok(RequestGuard::new(TlsTarget::new(host, 443)).with_substitution(Some(scope_for(fx, host)?)))
}

pub(crate) fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn post(host: &str, headers: &str, body: &str) -> Vec<u8> {
    format!(
        "POST /v1 HTTP/1.1\r\nHost: {host}\r\n{headers}Content-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

#[tokio::test]
async fn header_and_body_sentinels_are_replaced_at_the_matching_host() -> TestResult<()> {
    let fx = fixture()?;
    let request = post(
        API_HOST,
        &format!("Authorization: Bearer {}\r\n", fx.api),
        &format!("{{\"token\":\"{}\"}}", fx.api),
    );

    let outcome = exchange(guard_for(&fx, API_HOST)?, None, &request).await?;

    let received = text(&outcome.upstream);
    assert!(received.contains(&format!("Authorization: Bearer {API_REAL}\r\n")));
    assert_eq!(body_of(&outcome.upstream)?, format!("{{\"token\":\"{API_REAL}\"}}"));
    assert!(!received.contains("fake_value_"));
    assert!(text(&outcome.client).starts_with("HTTP/1.1 200"));
    Ok(())
}

pub(crate) fn body_of(upstream: &[u8]) -> TestResult<String> {
    let received = text(upstream);
    let (head, body) = received.split_once("\r\n\r\n").ok_or("expected a head")?;
    if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
        decode_chunked(body)
    } else {
        Ok(body.to_string())
    }
}

pub(crate) fn decode_chunked(body: &str) -> TestResult<String> {
    let mut decoded = String::new();
    let mut rest = body;
    loop {
        let (size, after) = rest.split_once("\r\n").ok_or("expected a chunk size")?;
        let size = usize::from_str_radix(size, 16)?;
        if size == 0 {
            assert_eq!(after, "\r\n");
            return Ok(decoded);
        }
        decoded.push_str(&after[..size]);
        assert_eq!(&after[size..size + 2], "\r\n");
        rest = &after[size + 2..];
    }
}

#[tokio::test]
async fn content_length_is_dropped_and_the_body_re_chunked_when_the_length_can_change() -> TestResult<()> {
    let fx = fixture()?;
    assert_ne!(fx.api.len(), API_REAL.len());
    let body = format!("a={}", fx.api);
    let request = post(API_HOST, "", &body);

    let outcome = exchange(guard_for(&fx, API_HOST)?, None, &request).await?;

    let received = text(&outcome.upstream);
    assert!(!received.to_ascii_lowercase().contains("content-length"));
    assert!(received.contains("Transfer-Encoding: chunked\r\n"));
    let (_, framed) = received.split_once("\r\n\r\n").ok_or("expected a value")?;
    assert_eq!(decode_chunked(framed)?, format!("a={API_REAL}"));
    Ok(())
}

#[tokio::test]
async fn content_length_stays_when_every_sentinel_keeps_its_length() -> TestResult<()> {
    let mut registry = SentinelRegistry::new();
    let real = "r".repeat(60);
    let sentinel = registry.register(
        SecretName::new("matched")?,
        RealValue::new(real.clone()),
        hosts(API_HOST)?,
    );
    assert_eq!(sentinel.to_string().len(), real.len());
    let substitution = SecretSubstitution::new(Arc::new(registry));
    let target = TlsTarget::new(API_HOST, 443);
    let guard = RequestGuard::new(target.clone()).with_substitution(substitution.scope_for(&target));
    let body = format!("a={sentinel}");
    let request = post(API_HOST, "", &body);

    let outcome = exchange(guard, None, &request).await?;

    let received = text(&outcome.upstream);
    assert!(received.contains(&format!("Content-Length: {}\r\n", body.len())));
    assert!(!received.contains("Transfer-Encoding"));
    assert!(received.ends_with(&format!("a={real}")));
    Ok(())
}

#[tokio::test]
async fn a_sentinel_split_across_http_chunks_is_replaced() -> TestResult<()> {
    let fx = fixture()?;
    let (first, second) = fx.api.split_at(17);
    let request = format!(
        "POST /v1 HTTP/1.1\r\nHost: {API_HOST}\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\nk={first}\r\n{:x}\r\n{second}&z=1\r\n0\r\n\r\n",
        first.len() + 2,
        second.len() + 4,
    );

    let outcome = exchange(guard_for(&fx, API_HOST)?, None, request.as_bytes()).await?;

    let received = text(&outcome.upstream);
    let (_, body) = received.split_once("\r\n\r\n").ok_or("expected a value")?;
    let mut decoded = String::new();
    let mut rest = body;
    loop {
        let (size, after) = rest.split_once("\r\n").ok_or("expected a value")?;
        let size = usize::from_str_radix(size, 16)?;
        if size == 0 {
            assert_eq!(after, "\r\n");
            break;
        }
        decoded.push_str(&after[..size]);
        assert_eq!(&after[size..size + 2], "\r\n");
        rest = &after[size + 2..];
    }
    assert_eq!(decoded, format!("k={API_REAL}&z=1"));
    assert!(received.contains("Transfer-Encoding: chunked\r\n"));
    Ok(())
}

#[tokio::test]
async fn a_sentinel_for_another_hosts_binding_is_left_as_is_and_reported_by_name() -> TestResult<()> {
    let fx = fixture()?;
    let observer = Arc::new(RecordingObserver::default());
    let substitution = fx.substitution.clone().with_observer(observer.clone());
    let target = TlsTarget::new(REGISTRY_HOST, 443);
    let guard = RequestGuard::new(target.clone()).with_substitution(substitution.scope_for(&target));
    let request = post(
        REGISTRY_HOST,
        &format!("X-Api-Key: {}\r\n", fx.api),
        &format!("registry={}&api={}", fx.registry, fx.api),
    );

    let outcome = exchange(guard, None, &request).await?;

    let received = text(&outcome.upstream);
    assert!(received.contains(&format!("X-Api-Key: {}\r\n", fx.api)));
    assert_eq!(
        body_of(&outcome.upstream)?,
        format!("registry={REGISTRY_REAL}&api={}", fx.api)
    );
    assert!(!received.contains(API_REAL));
    assert_eq!(
        *lock(&observer.seen)?,
        vec![
            (REGISTRY_HOST.to_string(), 443, "api".to_string()),
            (REGISTRY_HOST.to_string(), 443, "api".to_string()),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_spoofed_host_header_cannot_unlock_another_bindings_value() -> TestResult<()> {
    let fx = fixture()?;
    let request = post(API_HOST, &format!("X-Api-Key: {}\r\n", fx.api), "x");

    let outcome = exchange(guard_for(&fx, REGISTRY_HOST)?, None, &request).await?;

    assert!(outcome.upstream.is_empty());
    assert!(text(&outcome.client).starts_with("HTTP/1.1 421"));
    Ok(())
}

#[tokio::test]
async fn a_compressed_body_is_forwarded_unmodified_while_headers_are_still_substituted() -> TestResult<()> {
    let fx = fixture()?;
    let body = format!("not really gzip {}", fx.api);
    let request = post(
        API_HOST,
        &format!("Content-Encoding: gzip\r\nAuthorization: {}\r\n", fx.api),
        &body,
    );

    let outcome = exchange(guard_for(&fx, API_HOST)?, None, &request).await?;

    let received = text(&outcome.upstream);
    assert!(received.contains(&format!("Authorization: {API_REAL}\r\n")));
    assert_eq!(body_of(&outcome.upstream)?, body);
    assert!(received.contains(&format!("Content-Length: {}\r\n", body.len())));
    Ok(())
}

#[tokio::test]
async fn an_identity_content_encoding_is_still_scanned() -> TestResult<()> {
    let fx = fixture()?;
    let request = post(API_HOST, "Content-Encoding: identity\r\n", &format!("a={}", fx.api));

    let outcome = exchange(guard_for(&fx, API_HOST)?, None, &request).await?;

    assert_eq!(body_of(&outcome.upstream)?, format!("a={API_REAL}"));
    Ok(())
}

#[tokio::test]
async fn a_body_that_grows_past_the_budget_is_refused_and_never_forwarded() -> TestResult<()> {
    let mut registry = SentinelRegistry::new();
    let inject_hosts = hosts(API_HOST)?;
    let sentinel = registry.register(
        SecretName::new("big")?,
        RealValue::new("small".to_string()),
        inject_hosts.clone(),
    );
    registry.register(SecretName::new("big")?, RealValue::new("x".repeat(400)), inject_hosts);
    let substitution = SecretSubstitution::new(Arc::new(registry));
    let target = TlsTarget::new(API_HOST, 443);
    let guard = RequestGuard::new(target.clone())
        .with_body_limit(BodyLimit::new(200))
        .with_substitution(substitution.scope_for(&target));
    let request = post(API_HOST, "", &sentinel.to_string());

    let outcome = exchange(guard, None, &request).await?;

    assert!(outcome.upstream.is_empty());
    assert!(text(&outcome.client).starts_with("HTTP/1.1 413"));
    Ok(())
}

#[tokio::test]
async fn a_credential_injected_by_a_transform_is_not_rescanned() -> TestResult<()> {
    let fx = fixture()?;
    let transform = CredentialTransform {
        match_rules: Vec::new(),
        credential: CredentialRequest {
            provider: CredentialProvider::Registry,
            pull_secret_id: PullSecretId::new("test-secret"),
            scope: CredentialScope::RegistryPull {
                repository_glob: "*".to_string(),
            },
            ttl_seconds: 60,
            requester: None,
        },
        injection: CredentialInjection::Header(HeaderName::new("X-Injected")?),
    };
    let source = FakeCredentialSource::new().with_token("test-secret", &fx.api);
    let request = post(API_HOST, &format!("X-Api-Key: {}\r\n", fx.api), "x");

    let outcome = exchange(guard_for(&fx, API_HOST)?, Some((&transform, &source)), &request).await?;

    let received = text(&outcome.upstream);
    assert!(received.contains(&format!("X-Api-Key: {API_REAL}\r\n")));
    assert!(received.contains(&format!("x-injected: {}\r\n", fx.api)));
    Ok(())
}

#[tokio::test]
async fn a_request_without_sentinels_keeps_its_body_though_the_framing_may_change() -> TestResult<()> {
    let fx = fixture()?;
    let request = post(API_HOST, "X-Trace: abc\r\n", "plain body");

    let outcome = exchange(guard_for(&fx, API_HOST)?, None, &request).await?;

    assert_eq!(body_of(&outcome.upstream)?, "plain body");
    assert!(text(&outcome.upstream).contains("X-Trace: abc\r\n"));
    Ok(())
}

#[tokio::test]
async fn a_bodyless_get_is_forwarded_with_no_framing_headers_added() -> TestResult<()> {
    let fx = fixture()?;
    let request = format!(
        "GET /v1 HTTP/1.1\r\nHost: {API_HOST}\r\nAuthorization: Bearer {}\r\n\r\n",
        fx.api
    );

    let outcome = exchange(guard_for(&fx, API_HOST)?, None, request.as_bytes()).await?;

    let received = text(&outcome.upstream);
    assert!(received.contains(&format!("Authorization: Bearer {API_REAL}\r\n")));
    let lowered = received.to_ascii_lowercase();
    assert!(!lowered.contains("content-length"));
    assert!(!lowered.contains("transfer-encoding"));
    assert!(received.ends_with("\r\n\r\n"));
    Ok(())
}

#[tokio::test]
async fn a_get_that_declares_a_body_still_has_it_substituted() -> TestResult<()> {
    let fx = fixture()?;
    let body = format!("q={}", fx.api);
    let request = format!(
        "GET /v1 HTTP/1.1\r\nHost: {API_HOST}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );

    let outcome = exchange(guard_for(&fx, API_HOST)?, None, request.as_bytes()).await?;

    assert_eq!(body_of(&outcome.upstream)?, format!("q={API_REAL}"));
    Ok(())
}

#[tokio::test]
async fn a_host_with_no_injectable_credentials_is_forwarded_byte_for_byte() -> TestResult<()> {
    let fx = fixture()?;
    let request = post(
        "other.example.net",
        &format!("Authorization: Bearer {}\r\n", fx.api),
        &format!("token={}", fx.api),
    );

    let outcome = exchange(guard_for(&fx, "other.example.net")?, None, &request).await?;

    assert_eq!(outcome.upstream, request);
    Ok(())
}

#[tokio::test]
async fn form_urlencoded_and_multipart_bodies_are_substituted_with_their_structure_intact() -> TestResult<()> {
    let fx = fixture()?;
    let form = format!("grant=client&token={}&x=1", fx.api);
    let outcome = exchange(
        guard_for(&fx, API_HOST)?,
        None,
        &post(API_HOST, "Content-Type: application/x-www-form-urlencoded\r\n", &form),
    )
    .await?;
    assert_eq!(
        body_of(&outcome.upstream)?,
        format!("grant=client&token={API_REAL}&x=1")
    );

    let multipart = format!(
        "--BOUND\r\nContent-Disposition: form-data; name=\"key\"\r\n\r\n{}\r\n--BOUND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\n\r\nplain\r\n--BOUND--\r\n",
        fx.api
    );
    let outcome = exchange(
        guard_for(&fx, API_HOST)?,
        None,
        &post(
            API_HOST,
            "Content-Type: multipart/form-data; boundary=BOUND\r\n",
            &multipart,
        ),
    )
    .await?;
    assert_eq!(body_of(&outcome.upstream)?, multipart.replace(&fx.api, API_REAL));
    Ok(())
}

#[tokio::test]
async fn any_header_name_carries_a_substituted_sentinel() -> TestResult<()> {
    let fx = fixture()?;
    let request = post(
        API_HOST,
        &format!("Private-Token: {}\r\nX-Custom-Auth: pre-{}-post\r\n", fx.api, fx.api),
        "",
    );

    let outcome = exchange(guard_for(&fx, API_HOST)?, None, &request).await?;

    let received = text(&outcome.upstream);
    assert!(received.contains(&format!("Private-Token: {API_REAL}\r\n")));
    assert!(received.contains(&format!("X-Custom-Auth: pre-{API_REAL}-post\r\n")));
    Ok(())
}

#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl io::Write for CapturedLogs {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("poisoned lock"))?
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for CapturedLogs {
    type Writer = CapturedLogs;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn tracing_output_never_contains_a_real_value() -> TestResult<()> {
    let logs = CapturedLogs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let fx = fixture()?;
    let observer = Arc::new(RecordingObserver::default());
    let substitution = fx.substitution.clone().with_observer(observer);

    let compressed = post(
        API_HOST,
        &format!("Content-Encoding: br\r\nAuthorization: {}\r\n", fx.api),
        &format!("z{}", fx.api),
    );
    exchange(guard_for(&fx, API_HOST)?, None, &compressed).await?;

    let target = TlsTarget::new(REGISTRY_HOST, 443);
    let off_host = RequestGuard::new(target.clone()).with_substitution(substitution.scope_for(&target));
    let misdirected = post(REGISTRY_HOST, "", &format!("{}{}", fx.api, fx.registry));
    exchange(off_host, None, &misdirected).await?;

    let refused = RequestGuard::new(TlsTarget::new(API_HOST, 443))
        .with_body_limit(BodyLimit::new(8))
        .with_substitution(Some(scope_for(&fx, API_HOST)?));
    exchange(refused, None, &post(API_HOST, "", &fx.api)).await?;

    let captured = text(&lock(&logs.0)?);
    assert!(!captured.is_empty());
    for secret in [API_REAL, REGISTRY_REAL] {
        assert!(!captured.contains(secret));
    }
    assert!(!captured.contains(&fx.api));
    assert!(!captured.contains(&fx.registry));
    Ok(())
}

fn binding(name: &str, mode: SecretMode, pull_secret_id: &str) -> TestResult<SecretBinding> {
    Ok(SecretBinding::new(SecretBindingParts {
        name: SecretName::new(name)?,
        target: SecretTarget::Env(EnvVarName::new(name.to_ascii_uppercase())?),
        mode,
        extract: None,
        mask_duplicates: MaskDuplicates::Disabled,
        on_extract_no_match: ExtractNoMatch::default(),
        strip_response_headers: ResponseHeaderStrip::default(),
        inject_hosts: hosts(API_HOST)?,
        inject_scope: Default::default(),
        source: CredentialRequest {
            provider: CredentialProvider::Registry,
            pull_secret_id: PullSecretId::new(pull_secret_id),
            scope: CredentialScope::RegistryPull {
                repository_glob: "*".to_string(),
            },
            ttl_seconds: 60,
            requester: None,
        },
    })?)
}

fn bindings(entries: Vec<SecretBinding>) -> TestResult<SecretBindings> {
    Ok(SecretBindings::new(entries)?)
}

#[tokio::test]
async fn a_masked_secret_access_key_without_an_access_key_id_warns_that_it_cannot_be_re_signed() -> TestResult<()> {
    let logs = CapturedLogs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let source = FakeCredentialSource::new().with_token("one", "value-one");
    let bindings = bindings(vec![binding("aws_secret_access_key", SecretMode::Mask, "one")?])?;

    load_sentinel_registry(&bindings, &source).await?;

    let captured = text(&lock(&logs.0)?);
    assert!(
        captured.contains(
            "AWS secret access key is masked but AWS_ACCESS_KEY_ID is not, so SigV4 requests cannot be re-signed"
        ),
        "{captured}"
    );
    assert!(captured.contains("aws_secret_access_key"));
    assert!(!captured.contains("value-one"));
    Ok(())
}

#[tokio::test]
async fn load_fetches_every_mask_binding_through_the_credential_source() -> TestResult<()> {
    let source = FakeCredentialSource::new().with_token("one", "value-one");
    let bindings = bindings(vec![
        binding("first", SecretMode::Mask, "one")?,
        binding("second", SecretMode::Deny, "missing")?,
    ])?;

    let registry = load_sentinel_registry(&bindings, &source).await?;

    assert_eq!(registry.len(), 1);
    assert_eq!(source.fetch_count(), 1);
    let host = trogon_sandbox_contract::Host::new(API_HOST)?;
    let pair = registry.pairs_for_host(&host, 443).next().ok_or("expected a pair")?;
    assert_eq!(pair.1.expose(), "value-one");
    Ok(())
}

#[tokio::test]
async fn load_refuses_an_empty_value_and_keeps_a_value_with_a_line_break_whole() -> TestResult<()> {
    let source = FakeCredentialSource::new().with_token("one", "");
    let result = load_sentinel_registry(&bindings(vec![binding("first", SecretMode::Mask, "one")?])?, &source).await;
    assert!(matches!(result, Err(SecretLoadError::EmptyValue(_))));

    let source = FakeCredentialSource::new().with_token("one", "tok-en\n");
    let registry =
        load_sentinel_registry(&bindings(vec![binding("first", SecretMode::Mask, "one")?])?, &source).await?;
    let host = trogon_sandbox_contract::Host::new(API_HOST)?;
    let pair = registry.pairs_for_host(&host, 443).next().ok_or("expected a pair")?;
    assert_eq!(pair.1.expose(), "tok-en\n");
    Ok(())
}

#[tokio::test]
async fn a_value_with_a_line_break_is_substituted_in_a_body_and_refused_in_a_header() -> TestResult<()> {
    let mut registry = SentinelRegistry::new();
    let sentinel = registry
        .register(
            SecretName::new("file")?,
            RealValue::new("tok-en\n".to_string()),
            hosts(API_HOST)?,
        )
        .to_string();
    let scope = SecretSubstitution::new(Arc::new(registry))
        .scope_for(&TlsTarget::new(API_HOST, 443))
        .ok_or("expected a value")?;
    let guard = || RequestGuard::new(TlsTarget::new(API_HOST, 443)).with_substitution(Some(scope.clone()));

    let in_body = post(API_HOST, "", &format!("t={sentinel}"));
    let outcome = exchange(guard(), None, &in_body).await?;
    assert_eq!(body_of(&outcome.upstream)?, "t=tok-en\n");

    let in_header = post(API_HOST, &format!("X-Token: {sentinel}\r\n"), "");
    let outcome = exchange(guard(), None, &in_header).await?;
    assert!(outcome.upstream.is_empty());
    assert!(text(&outcome.client).starts_with("HTTP/1.1 400"));
    Ok(())
}

#[tokio::test]
async fn load_failure_names_the_binding_and_never_the_value() -> TestResult<()> {
    let source = FakeCredentialSource::new();

    let err = load_sentinel_registry(&bindings(vec![binding("first", SecretMode::Mask, "one")?])?, &source)
        .await
        .err()
        .ok_or("expected a value")?;

    assert!(matches!(&err, SecretLoadError::Fetch { name, .. } if name.as_str() == "first"));
    Ok(())
}

#[test]
fn flagged_names_are_deduplicated() -> TestResult<()> {
    let fx = fixture()?;
    let scope = scope_for(&fx, API_HOST)?;
    let mut scanner = scope.scanner();
    let mut out = Vec::new();
    scanner.feed(format!("{0}{0}", fx.registry).as_bytes(), &mut out);
    scanner.finish(&mut out);

    let flagged: BTreeSet<String> = scanner.take_flagged().iter().map(ToString::to_string).collect();

    assert_eq!(flagged, BTreeSet::from(["registry".to_string()]));
    Ok(())
}
