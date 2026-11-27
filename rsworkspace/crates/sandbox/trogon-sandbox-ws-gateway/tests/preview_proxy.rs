mod support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::any;
use serde_json::{Value, json};
use support::{
    MockSandboxBackend, TestResult, api_key, call_ok, closed_address, connect_with_token, key_spec, serve,
    serve_router, session_token, test_state,
};
use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{GetSandboxDomainResponse, LeaseError, LeaseId, PortAccess, TenantId};
use trogon_sandbox_ws_gateway::{API_KEY_HEADER, AppState, PreviewClaims, PreviewKeys, PreviewState};

struct Harness {
    gateway: String,
    key: String,
    mock: Arc<MockSandboxBackend>,
    hits: Arc<AtomicUsize>,
    keys: PreviewKeys,
    upstream: std::net::SocketAddr,
}

async fn upstream_echo(hits: Arc<AtomicUsize>) -> TestResult<std::net::SocketAddr> {
    let app = Router::new().fallback(any(move |request: Request| {
        let hits = Arc::clone(&hits);
        async move {
            hits.fetch_add(1, Ordering::SeqCst);
            let method = request.method().to_string();
            let uri = request.uri().to_string();
            let has_key = request.headers().contains_key(API_KEY_HEADER);
            let access = request
                .headers()
                .get("x-access")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("-")
                .to_string();
            let body = axum::body::to_bytes(request.into_body(), 1 << 20)
                .await
                .unwrap_or_default();
            let mut response = format!(
                "{method} {uri} key={has_key} access={access} body={}",
                String::from_utf8_lossy(&body)
            )
            .into_response();
            if uri.contains("redirect-local") {
                *response.status_mut() = StatusCode::FOUND;
                response
                    .headers_mut()
                    .insert(header::LOCATION, HeaderValue::from_static("/landing?a=1#frag"));
            } else if uri.contains("redirect-base") {
                *response.status_mut() = StatusCode::FOUND;
                response
                    .headers_mut()
                    .insert(header::LOCATION, HeaderValue::from_static("/base/landing"));
            } else if uri.contains("redirect-root") {
                *response.status_mut() = StatusCode::FOUND;
                response
                    .headers_mut()
                    .insert(header::LOCATION, HeaderValue::from_static("/"));
            } else if uri.contains("redirect-away") {
                *response.status_mut() = StatusCode::FOUND;
                response.headers_mut().insert(
                    header::LOCATION,
                    HeaderValue::from_static("https://elsewhere.invalid/x"),
                );
            } else if uri.contains("redirect-opaque") {
                *response.status_mut() = StatusCode::FOUND;
                if let Ok(value) = HeaderValue::from_bytes(&[0xff, 0xfe]) {
                    response.headers_mut().insert(header::LOCATION, value);
                }
            }
            if uri.contains("redirect-outside") {
                *response.status_mut() = StatusCode::FOUND;
                response
                    .headers_mut()
                    .insert(header::LOCATION, HeaderValue::from_static("/outside"));
            }
            response
                .headers_mut()
                .insert("keep-alive", HeaderValue::from_static("timeout=5"));
            response
        }
    }));
    serve_router(app).await
}

async fn start_with(state_edit: impl FnOnce(&mut AppState)) -> TestResult<Harness> {
    let hits = Arc::new(AtomicUsize::new(0));
    let upstream = upstream_echo(Arc::clone(&hits)).await?;
    let mock = Arc::new(MockSandboxBackend::new());
    mock.set_domain_endpoint(&upstream.to_string());
    let key = api_key("preview");
    let mut state = test_state(&key, "tenant-a", mock.clone()).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let gateway = format!("http://{addr}");
    let spec = key_spec("k1", "ab");
    state.preview = Some(Arc::new(PreviewState::new(
        PreviewKeys::parse(&spec, None)?,
        &gateway,
        Duration::from_secs(60),
        Duration::from_secs(3600),
    )?));
    state_edit(&mut state);
    let app = trogon_sandbox_ws_gateway::router(state, vec![]);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok(Harness {
        gateway,
        key,
        mock,
        hits,
        keys: PreviewKeys::parse(&spec, None)?,
        upstream,
    })
}

async fn start() -> TestResult<Harness> {
    start_with(|_| {}).await
}

/// Mints a preview URL the way a client does: over the websocket tool.
async fn mint(harness: &Harness, lease: &str, port: u16) -> TestResult<Value> {
    let addr = harness.gateway.trim_start_matches("http://").parse()?;
    let token = session_token(addr, &harness.key).await?;
    let mut ws = connect_with_token(addr, &token).await?;
    call_ok(&mut ws, "sandbox_get_domain", json!({"lease_id": lease, "port": port})).await
}

fn token_of(url: &str) -> Option<String> {
    url.trim_end_matches('/').rsplit('/').next().map(str::to_string)
}

async fn status(client: &reqwest::Client, url: String) -> TestResult<StatusCode> {
    Ok(client.get(url).send().await?.status())
}

#[tokio::test]
async fn minted_url_proxies_http_to_the_resolved_upstream() -> TestResult {
    let harness = start().await?;
    let result = mint(&harness, "lease-1", 3000).await?;
    assert_eq!(result["outcome"], "available");
    let url = result["url"].as_str().ok_or("url")?.to_string();
    assert!(result["expires_at"].as_u64().is_some());
    assert!(url.starts_with(&format!("{}/v1/preview/lease-1/3000/", harness.gateway)));

    let client = reqwest::Client::new();
    let response = client.get(format!("{url}app/index.html?x=1")).send().await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    assert_eq!(
        response.text().await?,
        "GET /app/index.html?x=1 key=false access=- body="
    );
    assert_eq!(
        client.get(&url).send().await?.text().await?,
        "GET / key=false access=- body="
    );
    let posted = client
        .post(format!("{url}submit"))
        .header(API_KEY_HEADER, "should-not-leak")
        .body("payload")
        .send()
        .await?;
    assert_eq!(posted.text().await?, "POST /submit key=false access=- body=payload");
    let bare = client.get(url.trim_end_matches('/')).send().await?;
    assert_eq!(bare.text().await?, "GET / key=false access=- body=");
    Ok(())
}

#[tokio::test]
async fn invalid_tokens_never_reach_the_upstream() -> TestResult {
    let harness = start().await?;
    let url = mint(&harness, "lease-1", 3000).await?["url"]
        .as_str()
        .ok_or("url")?
        .to_string();
    let token = token_of(&url).ok_or("token")?;
    let client = reqwest::Client::new();
    let base = &harness.gateway;

    assert_eq!(
        status(&client, format!("{base}/v1/preview/lease-2/3000/{token}/")).await?,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status(&client, format!("{base}/v1/preview/lease-1/3001/{token}/")).await?,
        StatusCode::UNAUTHORIZED
    );
    let tampered = format!("{token}A");
    assert_eq!(
        status(&client, format!("{base}/v1/preview/lease-1/3000/{tampered}/")).await?,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status(&client, format!("{base}/v1/preview/lease-1/3000/garbage/")).await?,
        StatusCode::UNAUTHORIZED
    );

    let claims = |expires_at| -> TestResult<PreviewClaims> {
        Ok(PreviewClaims {
            tenant: TenantId::new("tenant-a")?,
            lease_id: LeaseId::new("lease-1"),
            port: 3000,
            expires_at,
        })
    };
    let expired = harness.keys.issue(&claims(1)?)?;
    let response = client
        .get(format!("{base}/v1/preview/lease-1/3000/{expired}/"))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.json::<Value>().await?["error"], "preview_token_expired");
    let ungranted = harness.keys.issue(&claims(u64::MAX / 2)?)?;
    let response = client
        .get(format!("{base}/v1/preview/lease-1/3000/{ungranted}/"))
        .send()
        .await?;
    assert_eq!(response.json::<Value>().await?["error"], "invalid_preview_token");
    let other = PreviewKeys::parse(&key_spec("other", "cd"), None)?;
    let foreign = other.issue(&claims(u64::MAX / 2)?)?;
    assert_eq!(
        status(&client, format!("{base}/v1/preview/lease-1/3000/{foreign}/")).await?,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        harness.hits.load(Ordering::SeqCst),
        0,
        "no rejected request may reach the upstream"
    );
    Ok(())
}

#[tokio::test]
async fn websocket_upgrade_is_refused_explicitly() -> TestResult {
    let harness = start().await?;
    let url = mint(&harness, "lease-1", 3000).await?["url"]
        .as_str()
        .ok_or("url")?
        .to_string();
    let response = reqwest::Client::new()
        .get(url)
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    Ok(())
}

#[tokio::test]
async fn domain_response_has_no_preview_fields_when_disabled() -> TestResult {
    let key = api_key("preview-off");
    let addr = serve(test_state(&key, "tenant-a", Arc::new(MockSandboxBackend::new())).await?).await?;
    let token = session_token(addr, &key).await?;
    let mut ws = connect_with_token(addr, &token).await?;
    let result = call_ok(
        &mut ws,
        "sandbox_get_domain",
        json!({"lease_id": "lease-1", "port": 3000}),
    )
    .await?;
    assert_eq!(result["outcome"], "available");
    assert!(result.get("url").is_none());
    assert!(result.get("expires_at").is_none());
    let response = reqwest::get(format!("http://{addr}/v1/preview/lease-1/3000/x/")).await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.json::<Value>().await?["error"], "preview_disabled");
    Ok(())
}

#[tokio::test]
async fn upstream_resolution_failures_map_to_distinct_statuses() -> TestResult {
    let harness = start().await?;
    let url = mint(&harness, "lease-1", 3000).await?["url"]
        .as_str()
        .ok_or("url")?
        .to_string();
    let client = reqwest::Client::new();
    let cases: Vec<(Result<GetSandboxDomainResponse, ClientError>, StatusCode, &str)> = vec![
        (
            Ok(GetSandboxDomainResponse::NotReady { waited_ms: 5 }),
            StatusCode::BAD_GATEWAY,
            "upstream_not_ready",
        ),
        (
            Ok(GetSandboxDomainResponse::PortNotExposed),
            StatusCode::NOT_FOUND,
            "port_not_exposed",
        ),
        (
            Ok(GetSandboxDomainResponse::Unsupported),
            StatusCode::NOT_IMPLEMENTED,
            "preview_unsupported",
        ),
        (
            Ok(GetSandboxDomainResponse::Denied(LeaseError::LeaseNotRunning {
                lease_id: LeaseId::new("lease-1"),
            })),
            StatusCode::FORBIDDEN,
            "preview_denied",
        ),
        (
            Err(ClientError::Denied(LeaseError::LeaseNotRunning {
                lease_id: LeaseId::new("lease-1"),
            })),
            StatusCode::FORBIDDEN,
            "preview_denied",
        ),
        (
            Err(ClientError::Timeout(Duration::from_secs(1))),
            StatusCode::BAD_GATEWAY,
            "upstream_resolution_failed",
        ),
    ];
    for (reply, expected, code) in cases {
        harness.mock.queue_domain_reply(reply);
        let response = client.get(&url).send().await?;
        assert_eq!(response.status(), expected, "{code}");
        assert_eq!(response.json::<Value>().await?["error"], code);
    }
    let closed = closed_address().await?;
    harness.mock.set_domain_endpoint(&closed.to_string());
    let unreachable = client.get(&url).send().await?;
    assert_eq!(unreachable.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(unreachable.json::<Value>().await?["error"], "upstream_unreachable");
    Ok(())
}

#[tokio::test]
async fn access_urls_and_headers_steer_the_upstream_request() -> TestResult {
    let harness = start().await?;
    let url = mint(&harness, "lease-1", 3000).await?["url"]
        .as_str()
        .ok_or("url")?
        .to_string();
    let access = PortAccess {
        base_url: format!("http://{}/base/", harness.upstream),
        headers: BTreeMap::from([
            ("x-access".to_string(), "granted".to_string()),
            ("bad header name".to_string(), "dropped".to_string()),
        ]),
    };
    harness.mock.queue_domain_reply(Ok(GetSandboxDomainResponse::Available {
        endpoint: "ignored.invalid:1".to_string(),
        url: None,
        expires_at: None,
        access: Some(access.clone()),
    }));
    let client = reqwest::Client::new();
    let response = client.get(format!("{url}page")).send().await?;
    assert!(
        response.headers().get("keep-alive").is_none(),
        "hop-by-hop headers stay upstream"
    );
    assert_eq!(response.text().await?, "GET /base/page key=false access=granted body=");

    let no_redirect = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let prefix = url
        .trim_start_matches(&harness.gateway)
        .trim_end_matches('/')
        .to_string();
    let location = |path: &str| {
        let client = no_redirect.clone();
        let url = format!("{url}{path}");
        async move {
            let response = client.get(url).send().await?;
            Ok::<_, reqwest::Error>(response.headers().get(header::LOCATION).cloned())
        }
    };
    let local = location("redirect-local").await?.ok_or("missing local")?;
    assert_eq!(local.to_str()?, format!("{prefix}/landing?a=1#frag"));
    let root = location("redirect-root").await?.ok_or("missing root")?;
    assert_eq!(root.to_str()?, format!("{prefix}/"));
    let away = location("redirect-away").await?.ok_or("missing away")?;
    assert_eq!(away.to_str()?, "https://elsewhere.invalid/x");
    let opaque = location("redirect-opaque").await?.ok_or("missing opaque")?;
    assert_eq!(opaque.as_bytes(), &[0xff, 0xfe]);

    harness.mock.queue_domain_reply(Ok(GetSandboxDomainResponse::Available {
        endpoint: "ignored.invalid:1".to_string(),
        url: None,
        expires_at: None,
        access: Some(access),
    }));
    let based = location("redirect-base").await?.ok_or("missing based")?;
    assert_eq!(based.to_str()?, format!("{prefix}/landing"));
    let outside = location("redirect-outside").await?.ok_or("missing outside")?;
    assert_eq!(outside.to_str()?, format!("{prefix}/outside"));
    Ok(())
}

#[tokio::test]
async fn lease_ids_are_percent_encoded_in_the_minted_url() -> TestResult {
    let harness = start().await?;
    let result = mint(&harness, "lease one/2", 3000).await?;
    let url = result["url"].as_str().ok_or("url")?.to_string();
    assert!(url.contains("/v1/preview/lease%20one%2F2/3000/"), "{url}");
    let response = reqwest::get(format!("{url}x")).await?;
    assert_eq!(response.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn a_failing_grant_store_is_a_server_error_not_an_open_door() -> TestResult {
    let harness = start_with(|state| state.sessions = Arc::new(support::FailingSessionStore)).await?;
    let claims = PreviewClaims {
        tenant: TenantId::new("tenant-a")?,
        lease_id: LeaseId::new("lease-1"),
        port: 3000,
        expires_at: u64::MAX / 2,
    };
    let token = harness.keys.issue(&claims)?;
    let response = reqwest::get(format!("{}/v1/preview/lease-1/3000/{token}/", harness.gateway)).await?;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response.json::<Value>().await?["error"], "session_store_failure");
    Ok(())
}

#[tokio::test]
async fn domain_waits_until_the_port_answers_or_time_runs_out() -> TestResult {
    let harness = start().await?;
    let addr = harness.gateway.trim_start_matches("http://").parse()?;
    let token = session_token(addr, &harness.key).await?;
    let mut ws = connect_with_token(addr, &token).await?;
    let wait = |ms: u64| json!({"lease_id": "lease-1", "port": 3000, "wait_ms": ms});

    let ready = call_ok(&mut ws, "sandbox_get_domain", wait(5000)).await?;
    assert_eq!(ready["outcome"], "available");

    let closed = closed_address().await?;
    harness.mock.set_domain_endpoint(&closed.to_string());
    let not_ready = call_ok(&mut ws, "sandbox_get_domain", wait(300)).await?;
    assert_eq!(not_ready["outcome"], "not_ready");
    assert!(not_ready["waited_ms"].as_u64().is_some());

    harness.mock.queue_domain_reply(Ok(GetSandboxDomainResponse::Available {
        endpoint: closed.to_string(),
        url: None,
        expires_at: None,
        access: None,
    }));
    harness.mock.queue_domain_reply(Ok(GetSandboxDomainResponse::Available {
        endpoint: harness.upstream.to_string(),
        url: None,
        expires_at: None,
        access: None,
    }));
    let moved = call_ok(&mut ws, "sandbox_get_domain", wait(5000)).await?;
    assert_eq!(moved["endpoint"], harness.upstream.to_string());

    harness
        .mock
        .queue_domain_reply(Ok(GetSandboxDomainResponse::PortNotExposed));
    let passthrough = call_ok(&mut ws, "sandbox_get_domain", wait(5000)).await?;
    assert_eq!(passthrough["outcome"], "port_not_exposed");

    harness.mock.queue_domain_reply(Ok(GetSandboxDomainResponse::Available {
        endpoint: closed.to_string(),
        url: None,
        expires_at: None,
        access: None,
    }));
    harness
        .mock
        .queue_domain_reply(Err(ClientError::Timeout(Duration::from_secs(1))));
    let failed = support::call(&mut ws, "sandbox_get_domain", wait(5000)).await?;
    assert!(failed["error"].is_object(), "{failed}");

    let gateway_style = Router::new().fallback(any(|| async { StatusCode::BAD_GATEWAY }));
    let proxy_like = serve_router(gateway_style).await?;
    harness.mock.set_domain_endpoint(&proxy_like.to_string());
    let bad_gateway = call_ok(&mut ws, "sandbox_get_domain", wait(300)).await?;
    assert_eq!(bad_gateway["outcome"], "not_ready");
    Ok(())
}
