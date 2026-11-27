use std::io;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

use async_trait::async_trait;
use trogon_sandbox_contract::{AmbiguousTargets, AwsCredentialPairBinding, DomainPattern, InjectHosts, SecretName};
use trogon_sandbox_contract::{EgressViolationReason, UnsupportedSigv4Shape};
use trogon_sandbox_protocol::{RealValue, SentinelRegistry};

use crate::aws_sigv4::{
    PayloadHash, SignedHeaders, SigningInput, SigningScope, UNSIGNED_PAYLOAD, header_value, parse_authorization, sign,
};
use crate::http_intercept::{BodyLimit, RequestGuard, TlsTarget, run_compiled_http_intercept};
use crate::secret_substitution::{SecretSubstitution, SentinelMisdirectionObserver};
use crate::sigv4_resign::{Sigv4BodyLimit, Sigv4Policies, Sigv4RefusalError as Refusal, UnsupportedSigv4Policy};
use crate::substitution_tests::{TestResult, exchange, lock, text};
use crate::upstream_leg::UpstreamLeg;

const HOST: &str = "svc.example.com";
const OTHER_HOST: &str = "other.example.org";
const DATE: &str = "20150830T123600Z";
const SCOPE: &str = "20150830/us-east-1/service/aws4_request";
const REAL_ACCESS_KEY_ID: &str = "AKIAREALEXAMPLE000001";
const REAL_SECRET: &str = "realSecretAccessKey/0123456789abcdefghijklmnop";
const LONG_REAL_SECRET: &str = "realSecretAccessKey/0123456789abcdefghijklmnopqrstuvwxyz";
const REAL_TOKEN: &str = "realSessionToken//0123456789abcdefghijklmnopqrstuvwxyz";

#[derive(Default)]
struct RecordingObserver {
    refused: Mutex<Vec<(String, String, EgressViolationReason)>>,
}

#[async_trait]
impl SentinelMisdirectionObserver for RecordingObserver {
    async fn sentinel_off_host(&self, _host: &str, _port: u16, _binding: &SecretName) {}

    async fn compressed_response(&self, _host: &str, _port: u16, _binding: &SecretName) {}

    async fn sigv4_refused(&self, host: &str, _port: u16, binding: &SecretName, reason: EgressViolationReason) {
        if let Ok(mut refused) = self.refused.lock() {
            refused.push((host.to_string(), binding.to_string(), reason));
        }
    }
}

struct Pair {
    substitution: SecretSubstitution,
    access_key_id: String,
    secret: String,
    token: String,
}

fn hosts(host: &str) -> TestResult<InjectHosts> {
    Ok(InjectHosts::new(vec![DomainPattern::new(host)?])?)
}

fn name(value: &str) -> TestResult<SecretName> {
    Ok(SecretName::new(value)?)
}

fn pair(with_token: bool) -> TestResult<Pair> {
    pair_with_secret(with_token, REAL_SECRET)
}

fn pair_with_secret(with_token: bool, real_secret: &str) -> TestResult<Pair> {
    Ok(pair_with_secret_and_body(with_token, real_secret, None)?.0)
}

fn pair_with_body_secret(with_token: bool, body_secret: Option<&str>) -> TestResult<(Pair, String)> {
    pair_with_secret_and_body(with_token, REAL_SECRET, body_secret)
}

fn pair_with_secret_and_body(
    with_token: bool,
    real_secret: &str,
    body_secret: Option<&str>,
) -> TestResult<(Pair, String)> {
    pair_with_everything(with_token, real_secret, body_secret, REAL_TOKEN)
}

fn pair_with_real_token(real_token: &str) -> TestResult<Pair> {
    Ok(pair_with_everything(true, REAL_SECRET, None, real_token)?.0)
}

fn pair_with_everything(
    with_token: bool,
    real_secret: &str,
    body_secret: Option<&str>,
    real_token: &str,
) -> TestResult<(Pair, String)> {
    let mut registry = SentinelRegistry::new();
    let body_sentinel = match body_secret {
        Some(real) => registry
            .register(name("api_token")?, RealValue::new(real.to_string()), hosts(HOST)?)
            .to_string(),
        None => String::new(),
    };
    let access_key_id = registry.register(
        name("aws_akid")?,
        RealValue::new(REAL_ACCESS_KEY_ID.to_string()),
        hosts(HOST)?,
    );
    let secret = registry.register(
        name("aws_secret")?,
        RealValue::new(real_secret.to_string()),
        hosts(HOST)?,
    );
    let token = registry.register(name("aws_token")?, RealValue::new(real_token.to_string()), hosts(HOST)?);
    let session_token = if with_token { Some(name("aws_token")?) } else { None };
    registry.link_aws_pair(&AwsCredentialPairBinding::new(
        name("aws_akid")?,
        name("aws_secret")?,
        session_token,
    )?)?;
    let pair = Pair {
        substitution: SecretSubstitution::new(Arc::new(registry)),
        access_key_id: access_key_id.to_string(),
        secret: secret.to_string(),
        token: token.to_string(),
    };
    Ok((pair, body_sentinel))
}

fn guard(substitution: &SecretSubstitution, host: &str) -> RequestGuard {
    let target = TlsTarget::new(host, 443);
    RequestGuard::new(target.clone()).with_substitution(substitution.scope_for(&target))
}

fn scope() -> SigningScope {
    SigningScope {
        date: "20150830".to_string(),
        region: "us-east-1".to_string(),
        service: "service".to_string(),
    }
}

fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

struct ClientRequest<'a> {
    method: &'a str,
    target: &'a str,
    host: &'a str,
    extra: Vec<(&'a str, String)>,
    signed: &'a [&'a str],
    body: &'a str,
    payload: PayloadHash,
    chunked: bool,
}

impl<'a> ClientRequest<'a> {
    fn new(signed: &'a [&'a str], body: &'a str) -> Self {
        Self {
            method: "POST",
            target: "/queue?Action=Send",
            host: HOST,
            extra: Vec::new(),
            signed,
            body,
            payload: PayloadHash::of(body.as_bytes()),
            chunked: false,
        }
    }

    fn header(mut self, name: &'a str, value: impl Into<String>) -> Self {
        self.extra.push((name, value.into()));
        self
    }

    fn render(&self, access_key_id: &str, secret: &str) -> TestResult<Vec<u8>> {
        let mut all = headers(&[("Host", self.host), ("X-Amz-Date", DATE)]);
        all.extend(self.extra.iter().map(|(name, value)| (name.to_string(), value.clone())));
        let authorization = sign(
            &SigningInput {
                method: self.method,
                target: self.target,
                headers: &all,
                signed_headers: &SignedHeaders::new(self.signed.iter().copied()),
                payload_hash: &self.payload,
                amz_date: DATE,
                scope: &scope(),
            },
            &RealValue::new(access_key_id.to_string()),
            &RealValue::new(secret.to_string()),
        )?;
        let mut head = format!("{} {} HTTP/1.1\r\n", self.method, self.target);
        for (name, value) in &all {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str(&format!("Authorization: {authorization}\r\n"));
        if self.chunked {
            let (first, second) = self.body.split_at(self.body.len() / 2);
            head.push_str(&format!(
                "Transfer-Encoding: chunked\r\n\r\n{:x}\r\n{first}\r\n{:x}\r\n{second}\r\n0\r\n\r\n",
                first.len(),
                second.len()
            ));
        } else {
            head.push_str(&format!("Content-Length: {}\r\n\r\n{}", self.body.len(), self.body));
        }
        Ok(head.into_bytes())
    }
}

struct Received {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn received(upstream: &[u8]) -> TestResult<Received> {
    let raw = text(upstream);
    let (head, body) = raw.split_once("\r\n\r\n").ok_or("expected a header terminator")?;
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().ok_or("expected a request line")?.split(' ');
    let method = request_line.next().ok_or("expected a method")?.to_string();
    let target = request_line.next().ok_or("expected a target")?.to_string();
    let mut headers = Vec::new();
    for line in lines {
        let (name, value) = line.split_once(": ").ok_or("expected a header")?;
        headers.push((name.to_string(), value.to_string()));
    }
    let body = if header_value(&headers, "transfer-encoding").is_some() {
        decode_chunked(body)?
    } else {
        body.as_bytes().to_vec()
    };
    Ok(Received {
        method,
        target,
        headers,
        body,
    })
}

fn decode_chunked(mut rest: &str) -> TestResult<Vec<u8>> {
    let mut decoded = Vec::new();
    loop {
        let (size, after) = rest.split_once("\r\n").ok_or("expected a chunk size")?;
        let size = usize::from_str_radix(size, 16)?;
        if size == 0 {
            return Ok(decoded);
        }
        decoded.extend_from_slice(after.as_bytes().get(..size).ok_or("chunk shorter than its size")?);
        rest = after.get(size + 2..).ok_or("chunk missing its terminator")?;
    }
}

/// What a service holding the real credential does: recompute the signature of
/// the request it received and compare it with the one it was sent.
fn verifies_with_real_key(request: &Received) -> TestResult<bool> {
    verifies_with_real_secret(request, REAL_SECRET)
}

fn verifies_with_real_secret(request: &Received, real_secret: &str) -> TestResult<bool> {
    let sent = header_value(&request.headers, "authorization").ok_or("expected an authorization header")?;
    let parsed = parse_authorization(sent).ok_or("expected a parsable authorization header")?;
    let payload = match header_value(&request.headers, "x-amz-content-sha256") {
        Some(UNSIGNED_PAYLOAD) => PayloadHash::Unsigned,
        _ => PayloadHash::of(&request.body),
    };
    let expected = sign(
        &SigningInput {
            method: &request.method,
            target: &request.target,
            headers: &request.headers,
            signed_headers: &parsed.signed_headers,
            payload_hash: &payload,
            amz_date: header_value(&request.headers, "x-amz-date").ok_or("expected an x-amz-date header")?,
            scope: &parsed.scope,
        },
        &RealValue::new(REAL_ACCESS_KEY_ID.to_string()),
        &RealValue::new(real_secret.to_string()),
    )?;
    Ok(expected == sent)
}

fn assert_no_sentinels(upstream: &[u8]) {
    assert!(!text(upstream).contains("fake_value_"));
}

fn refused_with(client: &[u8], status: &str) {
    let client = text(client);
    assert!(client.starts_with(&format!("HTTP/1.1 {status}")), "{client}");
    assert!(client.contains("X-Proxy-Error: blocked-by-egress-proxy\r\n"));
}

#[tokio::test]
async fn a_request_signed_with_the_fake_credential_verifies_with_the_real_one() -> Result<(), Box<dyn std::error::Error>>
{
    let fx = pair(false)?;
    let request = ClientRequest::new(&["host", "x-amz-date"], "{\"id\":1}").render(&fx.access_key_id, &fx.secret)?;

    let outcome = exchange(guard(&fx.substitution, HOST), None, &request).await?;

    let upstream = received(&outcome.upstream)?;
    assert!(verifies_with_real_key(&upstream)?);
    let authorization = header_value(&upstream.headers, "authorization").ok_or("expected a value")?;
    assert!(authorization.contains(&format!("Credential={REAL_ACCESS_KEY_ID}/{SCOPE}")));
    assert!(authorization.contains("SignedHeaders=host;x-amz-date"));
    assert_eq!(header_value(&upstream.headers, "x-amz-date"), Some(DATE));
    assert_eq!(upstream.body, b"{\"id\":1}");
    assert_no_sentinels(&outcome.upstream);
    assert!(text(&outcome.client).starts_with("HTTP/1.1 200"));
    Ok(())
}

#[tokio::test]
async fn the_session_token_is_substituted_and_signed() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(true)?;
    let request = ClientRequest::new(&["host", "x-amz-date", "x-amz-security-token"], "")
        .header("X-Amz-Security-Token", fx.token.clone())
        .render(&fx.access_key_id, &fx.secret)?;

    let outcome = exchange(guard(&fx.substitution, HOST), None, &request).await?;

    let upstream = received(&outcome.upstream)?;
    assert_eq!(
        header_value(&upstream.headers, "x-amz-security-token"),
        Some(REAL_TOKEN)
    );
    assert!(verifies_with_real_key(&upstream)?);
    assert_no_sentinels(&outcome.upstream);
    Ok(())
}

#[tokio::test]
async fn a_session_token_the_client_did_not_sign_is_added_and_signed() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(true)?;
    let request = ClientRequest::new(&["host", "x-amz-date"], "").render(&fx.access_key_id, &fx.secret)?;

    let outcome = exchange(guard(&fx.substitution, HOST), None, &request).await?;

    let upstream = received(&outcome.upstream)?;
    assert_eq!(
        header_value(&upstream.headers, "x-amz-security-token"),
        Some(REAL_TOKEN)
    );
    let authorization = header_value(&upstream.headers, "authorization").ok_or("expected a value")?;
    assert!(authorization.contains("SignedHeaders=host;x-amz-date;x-amz-security-token"));
    assert!(verifies_with_real_key(&upstream)?);
    Ok(())
}

#[tokio::test]
async fn the_body_hash_is_recomputed_after_substitution_changed_the_body() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let body = format!("{{\"secret\":\"{}\"}}", fx.secret);
    let request = ClientRequest::new(&["host", "x-amz-content-sha256", "x-amz-date"], &body)
        .header("X-Amz-Content-Sha256", PayloadHash::of(body.as_bytes()).as_str())
        .render(&fx.access_key_id, &fx.secret)?;

    let outcome = exchange(guard(&fx.substitution, HOST), None, &request).await?;

    let upstream = received(&outcome.upstream)?;
    assert_eq!(upstream.body, format!("{{\"secret\":\"{REAL_SECRET}\"}}").into_bytes());
    assert_eq!(
        header_value(&upstream.headers, "x-amz-content-sha256"),
        Some(PayloadHash::of(&upstream.body).as_str())
    );
    assert!(verifies_with_real_key(&upstream)?);
    Ok(())
}

#[tokio::test]
async fn an_unsigned_payload_stays_unsigned() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let mut request = ClientRequest::new(&["host", "x-amz-content-sha256", "x-amz-date"], "data")
        .header("X-Amz-Content-Sha256", UNSIGNED_PAYLOAD);
    request.payload = PayloadHash::Unsigned;
    let request = request.render(&fx.access_key_id, &fx.secret)?;

    let outcome = exchange(guard(&fx.substitution, HOST), None, &request).await?;

    let upstream = received(&outcome.upstream)?;
    assert_eq!(
        header_value(&upstream.headers, "x-amz-content-sha256"),
        Some(UNSIGNED_PAYLOAD)
    );
    assert!(verifies_with_real_key(&upstream)?);
    Ok(())
}

#[tokio::test]
async fn a_chunked_body_is_hashed_on_its_decoded_payload() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let body = format!("a={}&b=2", fx.secret);
    let mut request = ClientRequest::new(&["host", "x-amz-date"], &body);
    request.chunked = true;
    let request = request.render(&fx.access_key_id, &fx.secret)?;

    let outcome = exchange(guard(&fx.substitution, HOST), None, &request).await?;

    let upstream = received(&outcome.upstream)?;
    assert_eq!(upstream.body, format!("a={REAL_SECRET}&b=2").into_bytes());
    assert!(verifies_with_real_key(&upstream)?);
    Ok(())
}

#[tokio::test]
async fn a_query_and_path_that_need_encoding_still_verify() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let mut request = ClientRequest::new(&["host", "x-amz-date"], "");
    request.method = "GET";
    request.target = "/a%20b/c?z=2&a=1&a=%7E";
    let request = request.render(&fx.access_key_id, &fx.secret)?;

    let outcome = exchange(guard(&fx.substitution, HOST), None, &request).await?;

    assert!(!outcome.upstream.is_empty(), "{}", text(&outcome.client));
    assert!(verifies_with_real_key(&received(&outcome.upstream)?)?);
    Ok(())
}

#[tokio::test]
async fn a_streaming_payload_is_denied_and_never_reaches_the_upstream() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let observer = Arc::new(RecordingObserver::default());
    let substitution = fx.substitution.clone().with_observer(observer.clone());
    let request = ClientRequest::new(&["host", "x-amz-content-sha256", "x-amz-date"], "x")
        .header("X-Amz-Content-Sha256", "STREAMING-AWS4-HMAC-SHA256-PAYLOAD")
        .render(&fx.access_key_id, &fx.secret)?;

    let outcome = exchange(guard(&substitution, HOST), None, &request).await?;

    refused_with(&outcome.client, "403");
    assert!(outcome.upstream.is_empty());
    assert_eq!(
        *lock(&observer.refused)?,
        vec![(
            HOST.to_string(),
            "aws_akid".to_string(),
            EgressViolationReason::SecretSigv4Unsupported(UnsupportedSigv4Shape::Streaming)
        )]
    );
    Ok(())
}

#[tokio::test]
async fn a_presigned_url_is_denied_and_never_reaches_the_upstream() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let observer = Arc::new(RecordingObserver::default());
    let substitution = fx.substitution.clone().with_observer(observer.clone());
    let target = format!(
        "/obj?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential={}%2F20150830%2Fus-east-1%2Fservice%2Faws4_request&X-Amz-Signature=abcdef",
        fx.access_key_id
    );
    let request = format!("GET {target} HTTP/1.1\r\nHost: {HOST}\r\n\r\n");

    let outcome = exchange(guard(&substitution, HOST), None, request.as_bytes()).await?;

    refused_with(&outcome.client, "403");
    assert!(
        text(&outcome.client).contains("AWS SigV4 presigned request uses a masked credential but cannot be re-signed by the sandbox proxy; denied by policy. Set credentials.sigv4.presigned to \"passthrough\" to forward it unmodified (it will fail upstream: the signature covers the masked placeholder)."),
        "{}",
        text(&outcome.client)
    );
    assert!(outcome.upstream.is_empty());
    assert_eq!(
        lock(&observer.refused)?[0].2,
        EgressViolationReason::SecretSigv4Unsupported(UnsupportedSigv4Shape::Presigned)
    );
    Ok(())
}

#[tokio::test]
async fn sigv4a_is_denied_and_never_reaches_the_upstream() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let observer = Arc::new(RecordingObserver::default());
    let substitution = fx.substitution.clone().with_observer(observer.clone());
    let request = format!(
        "GET / HTTP/1.1\r\nHost: {HOST}\r\nX-Amz-Date: {DATE}\r\nAuthorization: AWS4-ECDSA-P256-SHA256 Credential={}/20150830/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=3045\r\n\r\n",
        fx.access_key_id
    );

    let outcome = exchange(guard(&substitution, HOST), None, request.as_bytes()).await?;

    refused_with(&outcome.client, "403");
    assert!(
        text(&outcome.client).contains("AWS SigV4 sigv4a request uses a masked credential but cannot be re-signed by the sandbox proxy; denied by policy. Set credentials.sigv4.sigv4a to \"passthrough\" to forward it unmodified (it will fail upstream: the signature covers the masked placeholder)."),
        "{}",
        text(&outcome.client)
    );
    assert!(outcome.upstream.is_empty());
    assert_eq!(
        lock(&observer.refused)?[0].2,
        EgressViolationReason::SecretSigv4Unsupported(UnsupportedSigv4Shape::Sigv4a)
    );
    Ok(())
}

#[tokio::test]
async fn a_passthrough_policy_forwards_the_unsupported_shape_with_plain_substitution()
-> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let substitution = fx.substitution.clone().with_sigv4_policies(Sigv4Policies {
        streaming: UnsupportedSigv4Policy::Passthrough,
        ..Sigv4Policies::default()
    });
    let request = ClientRequest::new(&["host", "x-amz-content-sha256", "x-amz-date"], "x")
        .header("X-Amz-Content-Sha256", "STREAMING-AWS4-HMAC-SHA256-PAYLOAD")
        .render(&fx.access_key_id, &fx.secret)?;

    let outcome = exchange(guard(&substitution, HOST), None, &request).await?;

    let upstream = received(&outcome.upstream)?;
    let authorization = header_value(&upstream.headers, "authorization").ok_or("expected a value")?;
    assert!(authorization.contains(&format!("Credential={REAL_ACCESS_KEY_ID}/")));
    assert!(text(&outcome.client).starts_with("HTTP/1.1 200"));
    Ok(())
}

#[tokio::test]
async fn a_passthrough_policy_does_not_loosen_the_other_shapes() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let substitution = fx.substitution.clone().with_sigv4_policies(Sigv4Policies {
        streaming: UnsupportedSigv4Policy::Passthrough,
        ..Sigv4Policies::default()
    });
    let request = format!(
        "GET / HTTP/1.1\r\nHost: {HOST}\r\nX-Amz-Date: {DATE}\r\nAuthorization: AWS4-ECDSA-P256-SHA256 Credential={}/20150830/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=3045\r\n\r\n",
        fx.access_key_id
    );

    let outcome = exchange(guard(&substitution, HOST), None, request.as_bytes()).await?;

    refused_with(&outcome.client, "403");
    Ok(())
}

#[tokio::test]
async fn a_non_aws_request_carrying_a_masked_sentinel_still_gets_plain_substitution()
-> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let request = format!(
        "POST /hook HTTP/1.1\r\nHost: {HOST}\r\nX-Api-Key: {}\r\nContent-Length: 0\r\n\r\n",
        fx.secret
    );

    let outcome = exchange(guard(&fx.substitution, HOST), None, request.as_bytes()).await?;

    let upstream = received(&outcome.upstream)?;
    assert_eq!(header_value(&upstream.headers, "x-api-key"), Some(REAL_SECRET));
    assert!(header_value(&upstream.headers, "authorization").is_none());
    assert!(text(&outcome.client).starts_with("HTTP/1.1 200"));
    Ok(())
}

#[tokio::test]
async fn a_literal_hash_body_substitution_composes_and_keeps_the_content_length()
-> Result<(), Box<dyn std::error::Error>> {
    let fx = pair_with_secret(false, LONG_REAL_SECRET)?;
    assert_eq!(fx.secret.len(), LONG_REAL_SECRET.len());
    let body = format!("{{\"secret\":\"{}\"}}", fx.secret);
    let request = ClientRequest::new(&["host", "x-amz-content-sha256", "x-amz-date"], &body)
        .header("X-Amz-Content-Sha256", PayloadHash::of(body.as_bytes()).as_str())
        .render(&fx.access_key_id, &fx.secret)?;

    let outcome = exchange(guard(&fx.substitution, HOST), None, &request).await?;

    let upstream = received(&outcome.upstream)?;
    assert_eq!(
        upstream.body,
        format!("{{\"secret\":\"{LONG_REAL_SECRET}\"}}").into_bytes()
    );
    assert_eq!(
        header_value(&upstream.headers, "content-length"),
        Some(body.len().to_string().as_str())
    );
    assert!(header_value(&upstream.headers, "transfer-encoding").is_none());
    assert!(verifies_with_real_secret(&upstream, LONG_REAL_SECRET)?);
    Ok(())
}

#[tokio::test]
async fn a_re_signed_body_that_changed_length_is_framed_to_match() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let body = format!("{{\"secret\":\"{}\"}}", fx.secret);
    let request = ClientRequest::new(&["host", "x-amz-content-sha256", "x-amz-date"], &body)
        .header("X-Amz-Content-Sha256", PayloadHash::of(body.as_bytes()).as_str())
        .render(&fx.access_key_id, &fx.secret)?;

    let outcome = exchange(guard(&fx.substitution, HOST), None, &request).await?;

    let upstream = received(&outcome.upstream)?;
    match header_value(&upstream.headers, "content-length") {
        Some(length) => assert_eq!(length, upstream.body.len().to_string()),
        None => assert!(header_value(&upstream.headers, "transfer-encoding").is_some()),
    }
    Ok(())
}

#[tokio::test]
async fn a_request_signed_with_an_unmasked_credential_is_left_alone() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let request =
        ClientRequest::new(&["host", "x-amz-date"], "body").render("AKIAUNRELATED0000001", "someOtherSecret")?;

    let outcome = exchange(guard(&fx.substitution, HOST), None, &request).await?;

    let sent = received(&request)?;
    let forwarded = received(&outcome.upstream)?;
    let framing =
        |name: &str| name.eq_ignore_ascii_case("content-length") || name.eq_ignore_ascii_case("transfer-encoding");
    let unframed = |request: &Received| -> Vec<(String, String)> {
        request
            .headers
            .iter()
            .filter(|(name, _)| !framing(name))
            .cloned()
            .collect()
    };
    assert_eq!(unframed(&forwarded), unframed(&sent));
    assert_eq!(forwarded.body, b"body");
    Ok(())
}

#[tokio::test]
async fn a_pair_that_injects_elsewhere_is_not_re_signed() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let request = {
        let mut request = ClientRequest::new(&["host", "x-amz-date"], "body");
        request.host = OTHER_HOST;
        request.render(&fx.access_key_id, &fx.secret)?
    };

    let outcome = exchange(guard(&fx.substitution, OTHER_HOST), None, &request).await?;

    assert_eq!(outcome.upstream, request);
    Ok(())
}

#[tokio::test]
async fn a_request_without_x_amz_date_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let request = format!(
        "GET / HTTP/1.1\r\nHost: {HOST}\r\nAuthorization: AWS4-HMAC-SHA256 Credential={}/{SCOPE}, SignedHeaders=host, Signature=00\r\n\r\n",
        fx.access_key_id
    );

    let outcome = exchange(guard(&fx.substitution, HOST), None, request.as_bytes()).await?;

    refused_with(&outcome.client, "403");
    assert!(
        text(&outcome.client).contains(
            "AWS SigV4 request uses a masked credential but carries no x-amz-date header; the proxy cannot re-sign it."
        ),
        "{}",
        text(&outcome.client)
    );
    assert!(outcome.upstream.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_request_missing_a_signed_header_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let request = format!(
        "GET / HTTP/1.1\r\nHost: {HOST}\r\nX-Amz-Date: {DATE}\r\nAuthorization: AWS4-HMAC-SHA256 Credential={}/{SCOPE}, SignedHeaders=host;x-amz-date;x-custom, Signature=00\r\n\r\n",
        fx.access_key_id
    );

    let outcome = exchange(guard(&fx.substitution, HOST), None, request.as_bytes()).await?;

    refused_with(&outcome.client, "403");
    assert!(text(&outcome.client).contains("AWS SigV4 request uses a masked credential but signed header \"x-custom\" is missing; the proxy cannot re-sign it."), "{}",
        text(&outcome.client));
    assert!(outcome.upstream.is_empty());
    Ok(())
}

#[tokio::test]
async fn two_authorization_headers_are_refused() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let authorization = format!(
        "Authorization: AWS4-HMAC-SHA256 Credential={}/{SCOPE}, SignedHeaders=host;x-amz-date, Signature=00\r\n",
        fx.access_key_id
    );
    let request = format!("GET / HTTP/1.1\r\nHost: {HOST}\r\nX-Amz-Date: {DATE}\r\n{authorization}{authorization}\r\n");

    let outcome = exchange(guard(&fx.substitution, HOST), None, request.as_bytes()).await?;

    refused_with(&outcome.client, "403");
    assert!(text(&outcome.client).contains("AWS SigV4 request uses a masked credential but its Authorization header is malformed; the proxy cannot re-sign it."), "{}",
        text(&outcome.client));
    assert!(outcome.upstream.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_body_over_the_limit_is_refused_instead_of_signed() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let target = TlsTarget::new(HOST, 443);
    let guard = RequestGuard::new(target.clone())
        .with_substitution(fx.substitution.scope_for(&target))
        .with_sigv4_body_limit(Sigv4BodyLimit::new(8));
    let request =
        ClientRequest::new(&["host", "x-amz-date"], "0123456789abcdef").render(&fx.access_key_id, &fx.secret)?;

    let outcome = exchange(guard, None, &request).await?;

    let client = text(&outcome.client);
    assert!(client.starts_with("HTTP/1.1 403"));
    assert!(client.contains("8-byte buffering limit"));
    assert!(client.contains("UNSIGNED-PAYLOAD"));
    assert!(outcome.upstream.is_empty());
    Ok(())
}

#[tokio::test]
async fn no_real_value_reaches_a_refusal_a_violation_or_a_debug_rendering() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(true)?;
    let observer = Arc::new(RecordingObserver::default());
    let substitution = fx.substitution.clone().with_observer(observer.clone());
    let request = ClientRequest::new(&["host", "x-amz-content-sha256", "x-amz-date"], "x")
        .header("X-Amz-Content-Sha256", "STREAMING-AWS4-HMAC-SHA256-PAYLOAD")
        .render(&fx.access_key_id, &fx.secret)?;

    let outcome = exchange(guard(&substitution, HOST), None, &request).await?;

    let mut visible = text(&outcome.client);
    visible.push_str(&format!("{:?}", lock(&observer.refused)?));
    visible.push_str(&serde_json::to_string(&lock(&observer.refused)?[0].2)?);
    visible.push_str(&format!("{substitution:?}"));
    for real in [REAL_ACCESS_KEY_ID, REAL_SECRET, REAL_TOKEN] {
        assert!(!visible.contains(real));
    }
    Ok(())
}

fn capped_guard(fx: &Pair, cap: usize) -> RequestGuard {
    let target = TlsTarget::new(HOST, 443);
    RequestGuard::new(target.clone())
        .with_substitution(fx.substitution.scope_for(&target))
        .with_sigv4_body_limit(Sigv4BodyLimit::new(cap))
}

#[test]
fn the_default_resign_body_cap_is_64_mib() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(Sigv4BodyLimit::DEFAULT.bytes(), 64 * 1024 * 1024);
    assert_eq!(Sigv4BodyLimit::default(), Sigv4BodyLimit::DEFAULT);
    Ok(())
}

#[tokio::test]
async fn a_body_exactly_at_the_cap_is_buffered_and_resigned() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let body = "x".repeat(1024);
    let request = ClientRequest::new(&["host", "x-amz-date"], &body).render(&fx.access_key_id, &fx.secret)?;

    let outcome = exchange(capped_guard(&fx, 1024), None, &request).await?;

    let upstream = received(&outcome.upstream)?;
    assert_eq!(upstream.body.len(), 1024);
    assert!(verifies_with_real_key(&upstream)?);
    Ok(())
}

#[tokio::test]
async fn a_body_one_byte_over_the_cap_is_denied_with_nothing_sent_upstream() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let body = "x".repeat(1025);
    let request = ClientRequest::new(&["host", "x-amz-date"], &body).render(&fx.access_key_id, &fx.secret)?;

    let outcome = exchange(capped_guard(&fx, 1024), None, &request).await?;

    let client = text(&outcome.client);
    assert!(client.starts_with("HTTP/1.1 403"), "{client}");
    assert!(client.contains("1024-byte buffering limit"));
    assert!(client.contains("UNSIGNED-PAYLOAD"));
    assert!(outcome.upstream.is_empty());
    Ok(())
}

#[tokio::test]
async fn an_over_cap_chunked_body_is_denied_the_same_way() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let body = "y".repeat(4096);
    let mut request = ClientRequest::new(&["host", "x-amz-date"], &body);
    request.chunked = true;
    let request = request.render(&fx.access_key_id, &fx.secret)?;

    let outcome = exchange(capped_guard(&fx, 1024), None, &request).await?;

    assert!(text(&outcome.client).starts_with("HTTP/1.1 403"));
    assert!(outcome.upstream.is_empty());
    Ok(())
}

#[tokio::test]
async fn an_unsigned_payload_is_not_held_to_the_resign_cap() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let body = "z".repeat(4096);
    let mut request = ClientRequest::new(&["host", "x-amz-content-sha256", "x-amz-date"], &body)
        .header("X-Amz-Content-Sha256", UNSIGNED_PAYLOAD);
    request.payload = PayloadHash::Unsigned;
    let request = request.render(&fx.access_key_id, &fx.secret)?;

    let outcome = exchange(capped_guard(&fx, 1024), None, &request).await?;

    let upstream = received(&outcome.upstream)?;
    assert_eq!(upstream.body.len(), 4096);
    assert!(verifies_with_real_key(&upstream)?);
    Ok(())
}

#[tokio::test]
async fn a_body_past_the_general_limit_but_under_the_resign_cap_is_resigned_intact()
-> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let body = "w".repeat(9 * 1024 * 1024);
    assert!(!BodyLimit::DEFAULT.allows(body.len()));
    let request = ClientRequest::new(&["host", "x-amz-date"], &body).render(&fx.access_key_id, &fx.secret)?;
    let guard = guard(&fx.substitution, HOST);

    let (client_side, proxy_client) = duplex(256 * 1024);
    let (proxy_upstream, mut upstream_side) = duplex(256 * 1024);
    let (mut client_read, mut client_write) = tokio::io::split(client_side);
    let writer = tokio::spawn(async move {
        client_write.write_all(&request).await?;
        client_write.shutdown().await?;
        io::Result::Ok(())
    });
    let upstream = tokio::spawn(async move {
        let mut seen = Vec::new();
        let mut buf = vec![0u8; 256 * 1024];
        let mut need = None;
        loop {
            let n = upstream_side.read(&mut buf).await?;
            if n == 0 {
                return io::Result::Ok(seen);
            }
            seen.extend_from_slice(&buf[..n]);
            if need.is_none()
                && let Some(end) = seen.windows(4).position(|w| w == b"\r\n\r\n")
            {
                let head = String::from_utf8_lossy(&seen[..end]).to_ascii_lowercase();
                let length: usize = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .ok_or_else(|| io::Error::other("expected a content-length header"))?
                    .trim()
                    .parse()
                    .map_err(io::Error::other)?;
                need = Some(end + 4 + length);
            }
            if need.is_some_and(|need| seen.len() >= need) {
                upstream_side
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .await?;
                return io::Result::Ok(seen);
            }
        }
    });
    run_compiled_http_intercept(proxy_client, UpstreamLeg::connected(proxy_upstream), None, &guard).await?;
    let mut client = Vec::new();
    client_read.read_to_end(&mut client).await?;
    writer.await??;
    let upstream = received(&upstream.await??)?;

    assert!(text(&client).starts_with("HTTP/1.1 200"));
    assert_eq!(upstream.body.len(), body.len());
    assert!(upstream.body.iter().all(|b| *b == b'w'));
    assert!(verifies_with_real_key(&upstream)?);
    Ok(())
}

async fn proxied(substitution: &SecretSubstitution, request: &[u8]) -> TestResult<Received> {
    let split = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("expected a header terminator")?
        + 4;
    let head_text = text(&request[..split]).replacen(" /", &format!(" http://{HOST}/"), 1);
    let mut head = crate::http_intercept::read_proxied_head(head_text.as_bytes(), AmbiguousTargets::Refuse)?;
    let scope = substitution
        .scope_for(&TlsTarget::new(HOST, 80))
        .ok_or("expected a substitution scope")?;
    let mut client = &request[split..];
    let mut buffered = Vec::new();
    let outgoing = crate::http_intercept::substitute_proxied(&mut head, &scope).await?;
    let mut body = Vec::new();
    match outgoing {
        Some(outgoing) => {
            crate::http_intercept::relay_scanned_body(
                &mut client,
                &mut buffered,
                &mut body,
                head.framing,
                outgoing,
                &scope,
            )
            .await?;
        }
        None => {
            crate::http_intercept::relay_request_body(&mut client, &mut buffered, &mut body, head.framing).await?;
        }
    }
    if outgoing == Some(crate::framing::Framing::Chunked) {
        body = decode_chunked(&String::from_utf8(body)?)?;
    }
    Ok(Received {
        method: head.method,
        target: head.target.replacen(&format!("http://{HOST}"), "", 1),
        headers: head.headers,
        body,
    })
}

#[tokio::test]
async fn a_plain_http_sigv4_request_is_substituted_and_not_re_signed() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(true)?;
    let request = ClientRequest::new(&["host", "x-amz-date"], "{\"id\":1}").render(&fx.access_key_id, &fx.secret)?;
    let sent_signature = {
        let sent = received(&request)?;
        let authorization = header_value(&sent.headers, "authorization").ok_or("expected a value")?;
        authorization
            .rsplit("Signature=")
            .next()
            .ok_or("expected a value")?
            .to_string()
    };

    let upstream = proxied(&fx.substitution, &request).await?;

    let authorization = header_value(&upstream.headers, "authorization").ok_or("expected a value")?;
    assert!(authorization.contains(&format!("Credential={REAL_ACCESS_KEY_ID}/")));
    assert!(authorization.ends_with(&sent_signature));
    assert!(!verifies_with_real_key(&upstream)?);
    assert_eq!(upstream.body, b"{\"id\":1}");
    Ok(())
}

#[tokio::test]
async fn a_plain_http_streaming_payload_is_forwarded_not_refused() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let request = ClientRequest::new(&["host", "x-amz-content-sha256", "x-amz-date"], "x")
        .header("X-Amz-Content-Sha256", "STREAMING-AWS4-HMAC-SHA256-PAYLOAD")
        .render(&fx.access_key_id, &fx.secret)?;

    assert!(proxied(&fx.substitution, &request).await.is_ok());
    Ok(())
}

fn presigned_target(access_key_id: &str) -> String {
    format!(
        "/key?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential={access_key_id}%2F20260715%2Fus-east-1%2Fservice%2Faws4_request&X-Amz-Date=20260715T000000Z&X-Amz-Expires=3600&X-Amz-SignedHeaders=host&X-Amz-Signature=ff00"
    )
}

fn get(target: &str, extra: &str) -> Vec<u8> {
    format!("GET {target} HTTP/1.1\r\nHost: {HOST}\r\n{extra}\r\n").into_bytes()
}

fn sigv4a_request(access_key_id: &str) -> Vec<u8> {
    get(
        "/",
        &format!(
            "Authorization: AWS4-ECDSA-P256-SHA256 Credential={access_key_id}/20260715/service/aws4_request, SignedHeaders=host;x-amz-date;x-amz-region-set, Signature=ff00\r\nX-Amz-Date: 20260715T000000Z\r\nX-Amz-Region-Set: *\r\n"
        ),
    )
}

fn streaming_request(access_key_id: &str) -> Vec<u8> {
    format!(
        "PUT /bucket/key HTTP/1.1\r\nHost: {HOST}\r\nAuthorization: AWS4-HMAC-SHA256 Credential={access_key_id}/20260715/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature=ff00\r\nX-Amz-Date: 20260715T000000Z\r\nX-Amz-Content-Sha256: STREAMING-AWS4-HMAC-SHA256-PAYLOAD\r\nContent-Encoding: aws-chunked\r\nContent-Length: 7\r\n\r\nchunked"
    )
    .into_bytes()
}

#[tokio::test]
async fn a_signed_hop_by_hop_header_fails_closed_and_the_next_request_still_works()
-> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let request = get(
        "/",
        &format!(
            "Authorization: AWS4-HMAC-SHA256 Credential={}/20260715/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date;x-custom, Signature=ff00\r\nX-Amz-Date: {DATE}\r\nConnection: x-custom\r\nX-Custom: value\r\n",
            fx.access_key_id
        ),
    );

    let outcome = exchange(guard(&fx.substitution, HOST), None, &request).await?;

    refused_with(&outcome.client, "403");
    assert!(text(&outcome.client).contains("x-custom"), "{}", text(&outcome.client));
    assert!(outcome.upstream.is_empty());

    let follow_up = ClientRequest::new(&["host", "x-amz-date"], "").render(&fx.access_key_id, &fx.secret)?;
    let outcome = exchange(guard(&fx.substitution, HOST), None, &follow_up).await?;
    assert!(text(&outcome.client).starts_with("HTTP/1.1 200"));
    Ok(())
}

#[tokio::test]
async fn a_non_aws_request_with_a_masked_sentinel_still_gets_plain_substitution()
-> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let request = get("/", &format!("X-Api-Key: {}\r\n", fx.access_key_id));

    let outcome = exchange(guard(&fx.substitution, HOST), None, &request).await?;

    let upstream = received(&outcome.upstream)?;
    assert!(text(&outcome.client).starts_with("HTTP/1.1 200"));
    assert_eq!(header_value(&upstream.headers, "x-api-key"), Some(REAL_ACCESS_KEY_ID));
    Ok(())
}

#[tokio::test]
async fn a_presigned_url_with_an_unmasked_key_id_is_not_denied() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let request = get(&presigned_target("AKIDUNRELATEDEXAMPLE"), "");

    let outcome = exchange(guard(&fx.substitution, HOST), None, &request).await?;

    assert!(text(&outcome.client).starts_with("HTTP/1.1 200"));
    assert!(!outcome.upstream.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_junk_authorization_header_does_not_bypass_the_presigned_policy() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let request = get(
        &presigned_target(&fx.access_key_id),
        "Authorization: Basic dXNlcjpwYXNz\r\n",
    );

    let outcome = exchange(guard(&fx.substitution, HOST), None, &request).await?;

    refused_with(&outcome.client, "403");
    assert!(text(&outcome.client).contains("presigned"));
    assert!(outcome.upstream.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_junk_authorization_header_with_an_unmasked_presigned_query_stays_untouched()
-> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let junk = "Basic dXNlcjpwYXNz";
    let request = get(
        &presigned_target("AKIDUNRELATEDEXAMPLE"),
        &format!("Authorization: {junk}\r\n"),
    );

    let outcome = exchange(guard(&fx.substitution, HOST), None, &request).await?;

    let upstream = received(&outcome.upstream)?;
    assert!(text(&outcome.client).starts_with("HTTP/1.1 200"));
    assert_eq!(header_value(&upstream.headers, "authorization"), Some(junk));
    assert!(upstream.target.contains("X-Amz-Signature=ff00"));
    Ok(())
}

fn passthrough_everything(fx: &Pair) -> SecretSubstitution {
    fx.substitution.clone().with_sigv4_policies(Sigv4Policies {
        streaming: UnsupportedSigv4Policy::Passthrough,
        presigned: UnsupportedSigv4Policy::Passthrough,
        sigv4a: UnsupportedSigv4Policy::Passthrough,
    })
}

#[tokio::test]
async fn a_passthrough_policy_forwards_streaming_without_re_signing() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let substitution = passthrough_everything(&fx);

    let outcome = exchange(guard(&substitution, HOST), None, &streaming_request(&fx.access_key_id)).await?;

    let upstream = received(&outcome.upstream)?;
    let authorization = header_value(&upstream.headers, "authorization").ok_or("expected a value")?;
    assert!(text(&outcome.client).starts_with("HTTP/1.1 200"));
    assert!(authorization.contains("Signature=ff00"));
    assert!(authorization.contains(&format!("Credential={REAL_ACCESS_KEY_ID}/")));
    Ok(())
}

#[tokio::test]
async fn a_passthrough_policy_forwards_a_presigned_url_without_re_signing() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let substitution = passthrough_everything(&fx);
    let request = get(&presigned_target(&fx.access_key_id), "");

    let outcome = exchange(guard(&substitution, HOST), None, &request).await?;

    let upstream = received(&outcome.upstream)?;
    assert!(text(&outcome.client).starts_with("HTTP/1.1 200"));
    assert!(upstream.target.contains("X-Amz-Signature=ff00"));
    Ok(())
}

#[tokio::test]
async fn a_passthrough_policy_forwards_sigv4a_without_re_signing() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let substitution = passthrough_everything(&fx);

    let outcome = exchange(guard(&substitution, HOST), None, &sigv4a_request(&fx.access_key_id)).await?;

    let upstream = received(&outcome.upstream)?;
    let authorization = header_value(&upstream.headers, "authorization").ok_or("expected a value")?;
    assert!(text(&outcome.client).starts_with("HTTP/1.1 200"));
    assert!(authorization.starts_with("AWS4-ECDSA-P256-SHA256"));
    assert!(authorization.contains("Signature=ff00"));
    Ok(())
}

#[tokio::test]
async fn a_length_matched_body_substitution_is_signed_and_keeps_its_content_length()
-> Result<(), Box<dyn std::error::Error>> {
    let real = format!("real-api-token-{}", "x".repeat(45));
    let (fx, sentinel) = pair_with_body_secret(false, Some(&real))?;
    assert_eq!(sentinel.len(), real.len());
    let body = format!("{{\"token\":\"{sentinel}\",\"note\":\"sentinel in body\"}}");
    let request = ClientRequest::new(&["host", "x-amz-date"], &body).render(&fx.access_key_id, &fx.secret)?;

    let outcome = exchange(guard(&fx.substitution, HOST), None, &request).await?;

    let upstream = received(&outcome.upstream)?;
    let upstream_body = String::from_utf8(upstream.body.clone())?;
    assert!(upstream_body.contains(&real));
    assert!(!upstream_body.contains(&sentinel));
    assert_eq!(
        header_value(&upstream.headers, "content-length"),
        Some(body.len().to_string().as_str())
    );
    assert!(verifies_with_real_key(&upstream)?);
    Ok(())
}

#[tokio::test]
async fn a_short_secret_body_is_re_framed_and_the_signature_and_length_track_it()
-> Result<(), Box<dyn std::error::Error>> {
    let real = "short-real-token";
    let (fx, sentinel) = pair_with_body_secret(false, Some(real))?;
    assert!(sentinel.len() > real.len());
    let body = format!("{{\"token\":\"{sentinel}\"}}");
    let request = ClientRequest::new(&["host", "x-amz-date"], &body).render(&fx.access_key_id, &fx.secret)?;

    let outcome = exchange(guard(&fx.substitution, HOST), None, &request).await?;

    let upstream = received(&outcome.upstream)?;
    let upstream_body = String::from_utf8(upstream.body.clone())?;
    assert!(upstream_body.contains(real));
    assert!(!upstream_body.contains(&sentinel));
    assert!(upstream.body.len() < body.len());
    assert_eq!(
        header_value(&upstream.headers, "content-length"),
        Some(upstream.body.len().to_string().as_str())
    );
    assert!(verifies_with_real_key(&upstream)?);
    Ok(())
}

fn plan_for(
    substitution: &SecretSubstitution,
    request: &[u8],
) -> TestResult<(crate::sigv4_resign::ResignPlan, Received)> {
    let sent = received(request)?;
    let scope = substitution
        .scope_for(&TlsTarget::new(HOST, 443))
        .ok_or("expected a scope")?;
    let plan = scope
        .plan_sigv4(&sent.headers, &sent.target)
        .map_err(|refusal| format!("{refusal:?}"))?
        .ok_or("expected a plan")?;
    Ok((plan, sent))
}

#[test]
fn every_refusal_names_its_violation_reason() -> Result<(), Box<dyn std::error::Error>> {
    let unsignable = EgressViolationReason::SecretSigv4Unsignable;
    for refusal in [
        Refusal::MalformedAuthorization,
        Refusal::MissingDate,
        Refusal::MissingSignedHeader("x".to_string()),
        Refusal::ResignFailed("x".to_string()),
        Refusal::BodyTooLarge(Sigv4BodyLimit::new(1)),
    ] {
        assert_eq!(refusal.violation_reason(), unsignable);
    }
    assert_eq!(
        Refusal::Streaming.violation_reason(),
        EgressViolationReason::SecretSigv4Unsupported(UnsupportedSigv4Shape::Streaming)
    );
    Ok(())
}

#[test]
fn a_plan_names_its_binding_and_says_whether_it_hashes_the_body() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let signed = ["host", "x-amz-date"];
    let request = ClientRequest::new(&signed, "body").render(&fx.access_key_id, &fx.secret)?;
    let (plan, _) = plan_for(&fx.substitution, &request)?;
    assert_eq!(plan.binding(), &name("aws_akid")?);
    assert!(plan.hashes_body());
    Ok(())
}

#[test]
fn applying_a_plan_keeps_a_single_authorization_header() -> Result<(), Box<dyn std::error::Error>> {
    let fx = pair(false)?;
    let signed = ["host", "x-amz-date"];
    let request = ClientRequest::new(&signed, "body").render(&fx.access_key_id, &fx.secret)?;
    let (plan, sent) = plan_for(&fx.substitution, &request)?;
    let mut forwarded = sent.headers.clone();
    forwarded.push(("authorization".to_string(), "stale".to_string()));
    plan.apply(&sent.method, &sent.target, &mut forwarded, b"body")?;
    let count = forwarded
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        .count();
    assert_eq!(count, 1);
    Ok(())
}

#[test]
fn a_session_token_a_header_cannot_carry_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let mut registry = SentinelRegistry::new();
    let access_key_id = registry.register(
        name("aws_akid")?,
        RealValue::new(REAL_ACCESS_KEY_ID.to_string()),
        hosts(HOST)?,
    );
    let secret = registry.register(
        name("aws_secret")?,
        RealValue::new(REAL_SECRET.to_string()),
        hosts(HOST)?,
    );
    registry.register(
        name("aws_token")?,
        RealValue::new("bad\ntoken".to_string()),
        hosts(HOST)?,
    );
    registry.link_aws_pair(&AwsCredentialPairBinding::new(
        name("aws_akid")?,
        name("aws_secret")?,
        Some(name("aws_token")?),
    )?)?;
    let substitution = SecretSubstitution::new(Arc::new(registry));
    let signed = ["host", "x-amz-date"];
    let request = ClientRequest::new(&signed, "body").render(access_key_id.as_str(), secret.as_str())?;
    let (plan, sent) = plan_for(&substitution, &request)?;
    let mut forwarded = sent.headers.clone();
    let result = plan.apply(&sent.method, &sent.target, &mut forwarded, b"body");
    assert!(matches!(
        result,
        Err(crate::sigv4_resign::Sigv4RefusalError::ResignFailed(_))
    ));
    Ok(())
}

#[tokio::test]
async fn a_real_session_token_a_header_cannot_carry_is_refused_and_reported() -> Result<(), Box<dyn std::error::Error>>
{
    let fx = pair_with_real_token("bad\ntoken")?;
    let request = ClientRequest::new(&["host", "x-amz-date"], "").render(&fx.access_key_id, &fx.secret)?;

    let outcome = exchange(guard(&fx.substitution, HOST), None, &request).await?;

    assert!(outcome.upstream.is_empty());
    assert!(text(&outcome.client).starts_with("HTTP/1.1 4"));
    Ok(())
}
