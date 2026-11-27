use super::tests::{TestError, build_test_forward_target_ca_and_leaf, patient_deadlines, root_store_from_pem, vetted};
use super::*;
use crate::header_mutator::HeaderMutator;
use crate::request_filter::{RequestDecision, RequestFilter, RequestView};
use crate::tls_ca::{PermittedDomains, install_default_crypto_provider};
use std::time::Duration;
use tokio::io::{DuplexStream, duplex};
use tokio::net::TcpListener;
use trogon_sandbox_contract::{ForwardUrl, Host, MatchRule, NetworkPolicy, RequestBodyCap, StringMatcher};

fn hook_rule() -> MatchRule {
    MatchRule {
        path: Some(StringMatcher::Exact("/hook".to_string())),
        methods: Vec::new(),
        query_string: Vec::new(),
        headers: Vec::new(),
    }
}

fn empty_tls() -> Arc<rustls::ClientConfig> {
    install_default_crypto_provider();
    Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth(),
    )
}

fn guard() -> RequestGuard {
    RequestGuard::new(TlsTarget::new("app.example.com", 443))
}

async fn drive(
    client: DuplexStream,
    leg: UpstreamLeg<DuplexStream>,
    guard: RequestGuard,
    forward_url: &str,
    tls: Arc<rustls::ClientConfig>,
) -> Result<Result<(u64, u64), TlsInterceptError>, TestError> {
    let forward = RequestForward {
        forward_url: ForwardUrl::new(forward_url)?,
        match_rules: vec![hook_rule()],
        ttl_seconds: 60,
    };
    let match_rules = CompiledMatchRules::compile(&forward.match_rules)?;
    let signer = OidcSigner::generate()?;
    let subject = SandboxId::new("sandbox-abc123")?;
    let tenant_id = TenantId::new("tenant-1")?;
    let lease_id = LeaseId::new("lease-1".to_string());
    Ok(run_forward_intercept(
        client,
        leg,
        "app.example.com",
        tls,
        ForwardContext {
            forward: &forward,
            signer: &signer,
            issuer: "sandbox-egress-proxy",
            subject: &subject,
            tenant_id: &tenant_id,
            lease_id: &lease_id,
            original_scheme: "https",
            original_port: 443,
            marking: UpstreamMarking::NotNeeded,
        },
        &match_rules,
        &guard,
    )
    .await)
}

async fn read_head<S: AsyncRead + Unpin>(stream: &mut S) -> io::Result<Vec<u8>> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).await?;
        head.push(byte[0]);
    }
    Ok(head)
}

fn get(path: &str, extra: &str) -> Vec<u8> {
    format!("GET {path} HTTP/1.1\r\nHost: app.example.com\r\n{extra}\r\n").into_bytes()
}

fn ok_answer() -> &'static [u8] {
    b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"
}

fn hook_url() -> &'static str {
    "https://127.0.0.1:1/hook"
}

/// An upstream that answers every request head with an empty 200 until the
/// proxy closes it.
async fn answering_upstream(mut peer: DuplexStream) -> io::Result<()> {
    while read_head(&mut peer).await.is_ok() {
        peer.write_all(ok_answer()).await?;
    }
    Ok(())
}

fn error_is_http(outcome: &Result<(u64, u64), TlsInterceptError>) -> bool {
    matches!(outcome, Err(TlsInterceptError::HttpIntercept(_)))
}

#[tokio::test(start_paused = true)]
async fn a_tunnel_that_gets_no_first_request_is_ended() -> Result<(), TestError> {
    let (_client, proxy_client) = duplex(1024);
    let (proxy_up, _up) = duplex(1024);
    let guard = guard()
        .with_keep_alive(Duration::from_secs(1))
        .with_first_head(FirstHead::counted_from(
            tokio::time::Instant::now(),
            Duration::from_secs(1),
        ));
    let outcome = drive(
        proxy_client,
        UpstreamLeg::connected(proxy_up),
        guard,
        hook_url(),
        empty_tls(),
    )
    .await?;
    assert!(error_is_http(&outcome));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_tunnel_left_idle_after_a_request_ends_cleanly() -> Result<(), TestError> {
    let (mut client, proxy_client) = duplex(64 * 1024);
    let (proxy_up, up) = duplex(64 * 1024);
    let guard = guard()
        .with_keep_alive(Duration::from_secs(1))
        .with_first_head(FirstHead::counted_from(
            tokio::time::Instant::now(),
            Duration::from_secs(60),
        ));
    let upstream = tokio::spawn(answering_upstream(up));
    let script = async {
        client.write_all(&get("/other", "")).await?;
        read_head(&mut client).await?;
        let mut rest = Vec::new();
        client.read_to_end(&mut rest).await?;
        Ok::<_, TestError>(())
    };
    let (outcome, scripted) = tokio::join!(
        drive(
            proxy_client,
            UpstreamLeg::connected(proxy_up),
            guard,
            hook_url(),
            empty_tls()
        ),
        script
    );
    scripted?;
    assert!(outcome?.is_ok());
    upstream.await??;
    Ok(())
}

struct DenyEveryRequest;

#[async_trait::async_trait]
impl RequestFilter for DenyEveryRequest {
    async fn decide(&self, _request: &RequestView<'_>) -> RequestDecision {
        RequestDecision::Deny
    }
}

#[tokio::test]
async fn a_denied_request_is_answered_and_the_tunnel_stays_open() -> Result<(), TestError> {
    let (mut client, proxy_client) = duplex(64 * 1024);
    let (proxy_up, _up) = duplex(1024);
    let filter: Arc<dyn RequestFilter> = Arc::new(DenyEveryRequest);
    let checks = RequestChecks::assembled(&NetworkPolicy::AllowAll, Some(&filter), None, false, None, None);
    let guard = guard().with_request_checks(checks);
    let script = async {
        client.write_all(&get("/other", "")).await?;
        let answer = read_head(&mut client).await?;
        client.shutdown().await?;
        Ok::<_, TestError>(answer)
    };
    let (outcome, answer) = tokio::join!(
        drive(
            proxy_client,
            UpstreamLeg::connected(proxy_up),
            guard,
            hook_url(),
            empty_tls()
        ),
        script
    );
    assert!(answer?.starts_with(b"HTTP/1.1 403"));
    assert!(outcome?.is_ok());
    Ok(())
}

#[tokio::test]
async fn a_matched_body_over_the_limit_is_refused() -> Result<(), TestError> {
    let (mut client, proxy_client) = duplex(64 * 1024);
    let (proxy_up, _up) = duplex(1024);
    let script = async {
        client
            .write_all(b"POST /hook HTTP/1.1\r\nHost: app.example.com\r\nContent-Length: 9000000\r\n\r\n")
            .await?;
        let answer = read_head(&mut client).await?;
        Ok::<_, TestError>(answer)
    };
    let (outcome, answer) = tokio::join!(
        drive(
            proxy_client,
            UpstreamLeg::connected(proxy_up),
            guard(),
            hook_url(),
            empty_tls()
        ),
        script
    );
    assert!(answer?.starts_with(b"HTTP/1.1 413"));
    assert!(outcome?.is_ok());
    Ok(())
}

#[tokio::test]
async fn a_matched_body_the_client_abandons_is_an_error() -> Result<(), TestError> {
    let (mut client, proxy_client) = duplex(64 * 1024);
    let (proxy_up, _up) = duplex(1024);
    let script = async {
        client
            .write_all(b"POST /hook HTTP/1.1\r\nHost: app.example.com\r\nContent-Length: 10\r\n\r\nabc")
            .await?;
        client.shutdown().await?;
        Ok::<_, TestError>(())
    };
    let (outcome, scripted) = tokio::join!(
        drive(
            proxy_client,
            UpstreamLeg::connected(proxy_up),
            guard(),
            hook_url(),
            empty_tls()
        ),
        script
    );
    scripted?;
    assert!(error_is_http(&outcome?));
    Ok(())
}

#[tokio::test]
async fn a_capped_tunnel_holds_the_request_body_before_forwarding() -> Result<(), TestError> {
    let (mut client, proxy_client) = duplex(64 * 1024);
    let (proxy_up, up) = duplex(64 * 1024);
    let guard = guard().with_body_limit(BodyLimit::new(1024));
    let upstream = tokio::spawn(answering_upstream(up));
    let script = async {
        client.write_all(&get("/other", "")).await?;
        read_head(&mut client).await?;
        client.shutdown().await?;
        Ok::<_, TestError>(())
    };
    let (outcome, scripted) = tokio::join!(
        drive(
            proxy_client,
            UpstreamLeg::connected(proxy_up),
            guard,
            hook_url(),
            empty_tls()
        ),
        script
    );
    scripted?;
    assert!(outcome?.is_ok());
    upstream.await??;
    Ok(())
}

#[tokio::test]
async fn an_unreachable_upstream_is_answered_with_a_bad_gateway() -> Result<(), TestError> {
    let (mut client, proxy_client) = duplex(64 * 1024);
    let leg: UpstreamLeg<DuplexStream> = UpstreamLeg::on_demand(Box::new(|| {
        Box::pin(async {
            Err(HttpInterceptError::UpstreamConnect(
                io::ErrorKind::ConnectionRefused.into(),
            ))
        })
    }));
    let script = async {
        client.write_all(&get("/other", "")).await?;
        let answer = read_head(&mut client).await?;
        client.shutdown().await?;
        Ok::<_, TestError>(answer)
    };
    let (outcome, answer) = tokio::join!(drive(proxy_client, leg, guard(), hook_url(), empty_tls()), script);
    assert!(answer?.starts_with(b"HTTP/1.1 502"));
    assert!(outcome?.is_ok());
    Ok(())
}

#[tokio::test]
async fn a_head_that_cannot_be_written_upstream_is_answered_and_the_leg_redialed() -> Result<(), TestError> {
    let (mut client, proxy_client) = duplex(64 * 1024);
    let leg: UpstreamLeg<DuplexStream> = UpstreamLeg::on_demand(Box::new(|| {
        Box::pin(async {
            let (dialed, peer) = duplex(64);
            drop(peer);
            Ok(dialed)
        })
    }));
    let script = async {
        client.write_all(&get("/other", "")).await?;
        let answer = read_head(&mut client).await?;
        client.shutdown().await?;
        Ok::<_, TestError>(answer)
    };
    let (outcome, answer) = tokio::join!(drive(proxy_client, leg, guard(), hook_url(), empty_tls()), script);
    assert!(answer?.starts_with(b"HTTP/1.1 502"));
    assert!(outcome?.is_ok());
    Ok(())
}

#[tokio::test]
async fn a_connected_upstream_that_cannot_be_written_ends_the_tunnel() -> Result<(), TestError> {
    let (mut client, proxy_client) = duplex(64 * 1024);
    let (proxy_up, up) = duplex(64);
    drop(up);
    let script = async {
        client.write_all(&get("/other", "")).await?;
        let mut rest = Vec::new();
        client.read_to_end(&mut rest).await?;
        Ok::<_, TestError>(rest)
    };
    let (outcome, rest) = tokio::join!(
        drive(
            proxy_client,
            UpstreamLeg::connected(proxy_up),
            guard(),
            hook_url(),
            empty_tls()
        ),
        script
    );
    assert!(rest?.starts_with(b"HTTP/1.1 502"));
    assert!(error_is_http(&outcome?));
    Ok(())
}

#[tokio::test]
async fn a_reused_upstream_that_went_away_ends_the_tunnel_silently() -> Result<(), TestError> {
    let (mut client, proxy_client) = duplex(64 * 1024);
    let (proxy_up, mut up) = duplex(64 * 1024);
    let upstream = tokio::spawn(async move {
        read_head(&mut up).await?;
        up.write_all(ok_answer()).await
    });
    let script = async {
        client.write_all(&get("/other", "")).await?;
        read_head(&mut client).await?;
        upstream.await??;
        client.write_all(&get("/other", "")).await?;
        let mut rest = Vec::new();
        client.read_to_end(&mut rest).await?;
        Ok::<_, TestError>(rest)
    };
    let (outcome, rest) = tokio::join!(
        drive(
            proxy_client,
            UpstreamLeg::connected(proxy_up),
            guard(),
            hook_url(),
            empty_tls()
        ),
        script
    );
    assert!(rest?.is_empty());
    assert!(error_is_http(&outcome?));
    Ok(())
}

#[tokio::test]
async fn a_client_body_that_ends_early_fails_the_exchange() -> Result<(), TestError> {
    let (mut client, proxy_client) = duplex(64 * 1024);
    let (proxy_up, mut up) = duplex(64 * 1024);
    let upstream = tokio::spawn(async move {
        let mut sink = Vec::new();
        up.read_to_end(&mut sink).await
    });
    let script = async {
        client
            .write_all(b"POST /other HTTP/1.1\r\nHost: app.example.com\r\nContent-Length: 10\r\n\r\nabc")
            .await?;
        client.shutdown().await?;
        Ok::<_, TestError>(())
    };
    let (outcome, scripted) = tokio::join!(
        drive(
            proxy_client,
            UpstreamLeg::connected(proxy_up),
            guard(),
            hook_url(),
            empty_tls()
        ),
        script
    );
    scripted?;
    assert!(error_is_http(&outcome?));
    upstream.await??;
    Ok(())
}

#[tokio::test]
async fn a_malformed_chunked_body_ends_the_exchange_after_the_head_went_upstream() -> Result<(), TestError> {
    let (mut client, proxy_client) = duplex(64 * 1024);
    let (proxy_up, mut up) = duplex(64 * 1024);
    let upstream = tokio::spawn(async move {
        let mut sink = Vec::new();
        up.read_to_end(&mut sink).await
    });
    let script = async {
        client
            .write_all(
                b"POST /other HTTP/1.1\r\nHost: app.example.com\r\nTransfer-Encoding: chunked\r\n\r\nZZ\r\nabc\r\n",
            )
            .await?;
        let mut rest = Vec::new();
        client.read_to_end(&mut rest).await?;
        Ok::<_, TestError>(rest)
    };
    let (outcome, answer) = tokio::join!(
        drive(
            proxy_client,
            UpstreamLeg::connected(proxy_up),
            guard(),
            hook_url(),
            empty_tls()
        ),
        script
    );
    assert!(answer?.is_empty());
    assert!(outcome?.is_ok());
    upstream.await??;
    Ok(())
}

#[tokio::test]
async fn an_unmatched_request_asking_to_close_ends_the_tunnel() -> Result<(), TestError> {
    let (mut client, proxy_client) = duplex(64 * 1024);
    let (proxy_up, up) = duplex(64 * 1024);
    let upstream = tokio::spawn(answering_upstream(up));
    let script = async {
        client.write_all(&get("/other", "Connection: close\r\n")).await?;
        read_head(&mut client).await?;
        let mut rest = Vec::new();
        client.read_to_end(&mut rest).await?;
        Ok::<_, TestError>(())
    };
    let (outcome, scripted) = tokio::join!(
        drive(
            proxy_client,
            UpstreamLeg::connected(proxy_up),
            guard(),
            hook_url(),
            empty_tls()
        ),
        script
    );
    scripted?;
    assert!(outcome?.is_ok());
    upstream.await??;
    Ok(())
}

#[tokio::test]
async fn a_mutator_that_breaks_the_headers_gets_the_request_refused() -> Result<(), TestError> {
    let (mut client, proxy_client) = duplex(64 * 1024);
    let (proxy_up, _up) = duplex(1024);
    let mutator: Arc<dyn HeaderMutator> = Arc::new(|headers: &mut Vec<(String, String)>, _host: &Host| {
        headers.push(("not a header".to_string(), "x".to_string()));
    });
    let checks = RequestChecks::assembled(&NetworkPolicy::AllowAll, None, None, false, Some(&mutator), None);
    let guard = guard().with_request_checks(checks);
    let script = async {
        client.write_all(&get("/other", "")).await?;
        let answer = read_head(&mut client).await?;
        Ok::<_, TestError>(answer)
    };
    let (outcome, answer) = tokio::join!(
        drive(
            proxy_client,
            UpstreamLeg::connected(proxy_up),
            guard,
            hook_url(),
            empty_tls()
        ),
        script
    );
    assert!(answer?.starts_with(b"HTTP/1.1 403"));
    assert!(outcome?.is_ok());
    Ok(())
}

#[tokio::test]
async fn matched_requests_share_one_forward_connection() -> Result<(), TestError> {
    install_default_crypto_provider();
    let forward_ca = build_test_forward_target_ca_and_leaf("127.0.0.1".parse()?)?;
    let server_config = Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![forward_ca.cert.clone()], forward_ca.key.clone_key())?,
    );
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let accepted = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await?;
        let mut tls = TlsAcceptor::from(server_config).accept(tcp).await?;
        let mut served = 0u32;
        while read_head(&mut tls).await.is_ok() {
            served += 1;
            tls.write_all(ok_answer()).await?;
            tls.flush().await?;
        }
        io::Result::Ok(served)
    });
    let tls = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store_from_pem(&forward_ca.pem)?)
            .with_no_client_auth(),
    );
    let (mut client, proxy_client) = duplex(64 * 1024);
    let (proxy_up, _up) = duplex(1024);
    let script = async {
        client.write_all(&get("/hook", "")).await?;
        read_head(&mut client).await?;
        client.write_all(&get("/hook", "Connection: close\r\n")).await?;
        read_head(&mut client).await?;
        let mut rest = Vec::new();
        client.read_to_end(&mut rest).await?;
        Ok::<_, TestError>(())
    };
    let url = format!("https://127.0.0.1:{port}/hook");
    let (outcome, scripted) = tokio::join!(
        drive(proxy_client, UpstreamLeg::connected(proxy_up), guard(), &url, tls),
        script
    );
    scripted?;
    assert!(outcome?.is_ok());
    assert_eq!(accepted.await??, 2);
    Ok(())
}

#[tokio::test]
async fn forward_targets_that_cannot_be_used_are_reported() -> Result<(), TestError> {
    let tls = empty_tls();
    let unparsable = connect_forward_target("not a url", &tls, UpstreamMarking::NotNeeded).await;
    assert!(matches!(unparsable, Err(TlsInterceptError::InvalidForwardUrl { .. })));
    let hostless = connect_forward_target("data:text/plain,hi", &tls, UpstreamMarking::NotNeeded).await;
    assert!(matches!(hostless, Err(TlsInterceptError::InvalidForwardUrl { .. })));
    let invalid_name = connect_forward_target("https://a!b.test/", &tls, UpstreamMarking::NotNeeded).await;
    assert!(matches!(invalid_name, Err(TlsInterceptError::InvalidServerName { .. })));
    let closed = TcpListener::bind("127.0.0.1:0").await?;
    let port = closed.local_addr()?.port();
    drop(closed);
    let url = format!("https://127.0.0.1:{port}/");
    let refused = connect_forward_target(&url, &tls, UpstreamMarking::NotNeeded).await;
    assert!(matches!(refused, Err(TlsInterceptError::ForwardConnect { .. })));
    Ok(())
}

fn probe_ca() -> Result<SandboxCa, TestError> {
    install_default_crypto_provider();
    Ok(SandboxCa::generate(PermittedDomains::for_tests(&["*.example.com"]))?)
}

#[tokio::test]
async fn a_client_that_does_not_start_with_a_handshake_record_is_refused() -> Result<(), TestError> {
    let ca = probe_ca()?;
    let (mut peer, client) = duplex(1024);
    peer.write_all(b"GET / HTTP/1.1\r\n").await?;
    let options = NetworkOptions {
        request_body_cap: Some(RequestBodyCap::new(1024)?),
        ..NetworkOptions::default()
    };
    let outcome = terminate_and_forward_checked(
        PrefixedStream::new(client, vec![]),
        "app.example.com",
        &ca,
        vetted("127.0.0.1:9".parse()?).into(),
        UpstreamMarking::NotNeeded,
        empty_tls(),
        None,
        None,
        None,
        patient_deadlines(),
        None,
        None,
        None,
        Some(&options),
    )
    .await;
    assert!(matches!(outcome, Err(TlsInterceptError::ClientHandshake(_))));
    Ok(())
}

#[tokio::test]
async fn a_target_that_is_not_a_server_name_is_refused() -> Result<(), TestError> {
    let ca = probe_ca()?;
    let (mut peer, client) = duplex(1024);
    peer.write_all(b"GET / HTTP/1.1\r\n").await?;
    let outcome = terminate_and_forward_checked(
        PrefixedStream::new(client, vec![]),
        "bad host",
        &ca,
        vetted("127.0.0.1:9".parse()?).into(),
        UpstreamMarking::NotNeeded,
        empty_tls(),
        None,
        None,
        None,
        patient_deadlines(),
        None,
        None,
        None,
        None,
    )
    .await;
    assert!(matches!(outcome, Err(TlsInterceptError::InvalidServerName { .. })));
    Ok(())
}
