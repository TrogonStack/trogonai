use std::time::Duration;

use serde_json::{Value, json};
use url::Url;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::snapshots::SnapshotTemplateId;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const BUILD: &str = "11111111-2222-4333-8444-555555555555";

fn api(server: &MockServer) -> Result<E2bApi, Box<dyn std::error::Error>> {
    let config = E2bConfig::new(Url::parse(&server.uri())?, DEFAULT_DOMAIN, "test-key")?;
    Ok(E2bApi::new(config, Duration::from_secs(5))?)
}

fn valid_template() -> Value {
    json!({
        "templateID": "owned-1",
        "buildID": BUILD,
        "buildCount": 1,
        "builds": [{"buildID": BUILD, "status": "ready"}],
    })
}

async fn capture(server: &MockServer, reply: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path("/sandboxes/source/snapshots"))
        .and(body_json(json!({"memory": true})))
        .respond_with(reply)
        .mount(server)
        .await;
}

async fn template(server: &MockServer, reply: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path("/templates/owned-1"))
        .respond_with(reply)
        .mount(server)
        .await;
}

async fn created(server: &MockServer) -> Result<(E2bApi, CreatedSnapshot), Box<dyn std::error::Error>> {
    capture(
        server,
        ResponseTemplate::new(201).set_body_json(json!({"snapshotID": "owned-1:default", "names": []})),
    )
    .await;
    let api = api(server)?;
    let created = api.create_snapshot("source").await?;
    Ok((api, created))
}

fn replace(value: &mut Value, pointer: &str, replacement: Value) -> TestResult {
    *value.pointer_mut(pointer).ok_or("pointer is absent")? = replacement;
    Ok(())
}

#[tokio::test]
async fn ambiguous_snapshot_resolution_never_changes_the_owned_cleanup_target() -> TestResult {
    let server = MockServer::start().await;
    let (api, created) = created(&server).await?;
    Mock::given(method("DELETE"))
        .and(path("/templates/owned-1"))
        .respond_with(ResponseTemplate::new(204))
        .expect(5)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/templates/borrowed"))
        .respond_with(ResponseTemplate::new(204))
        .expect(0)
        .mount(&server)
        .await;
    for (pointer, replacement) in [
        ("/templateID", json!("borrowed")),
        ("/buildID", json!("not-a-build")),
        ("/builds/0/buildID", json!("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee")),
        ("/builds/0/status", json!("building")),
        ("/buildCount", json!(2)),
    ] {
        let mut response = valid_template();
        replace(&mut response, pointer, replacement)?;
        server.reset().await;
        template(&server, ResponseTemplate::new(200).set_body_json(response)).await;
        Mock::given(method("DELETE"))
            .and(path("/templates/owned-1"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        assert!(api.resolve_snapshot(&created).await.is_err());
        api.delete_snapshot(&created.template_id).await?;
    }
    Ok(())
}

#[tokio::test]
async fn a_template_with_two_builds_is_not_a_unique_ready_build() -> TestResult {
    let server = MockServer::start().await;
    let (api, created) = created(&server).await?;
    let mut response = valid_template();
    replace(
        &mut response,
        "/builds",
        json!([{"buildID": BUILD, "status": "ready"}, {"buildID": BUILD, "status": "ready"}]),
    )?;
    template(&server, ResponseTemplate::new(200).set_body_json(response)).await;
    assert!(matches!(
        api.resolve_snapshot(&created).await,
        Err(ApiError::Transport(_))
    ));
    Ok(())
}

#[tokio::test]
async fn a_confirmed_template_resolves_to_a_binding() -> TestResult {
    let server = MockServer::start().await;
    let (api, created) = created(&server).await?;
    template(&server, ResponseTemplate::new(200).set_body_json(valid_template())).await;
    let binding = api.resolve_snapshot(&created).await?;
    assert_eq!(binding.template_id, created.template_id);
    assert_eq!(String::from(binding.build_id), BUILD);
    Ok(())
}

#[tokio::test]
async fn an_oversized_template_reply_is_refused() -> TestResult {
    let server = MockServer::start().await;
    let (api, created) = created(&server).await?;
    template(
        &server,
        ResponseTemplate::new(200).set_body_string("x".repeat(constants::MAX_RESOLVE_SNAPSHOT_REPLY_BYTES + 1)),
    )
    .await;
    assert!(matches!(
        api.resolve_snapshot(&created).await,
        Err(ApiError::Transport(_))
    ));
    Ok(())
}

#[tokio::test]
async fn snapshot_creation_refuses_every_reply_without_a_canonical_identity() -> TestResult {
    for reply in [
        ResponseTemplate::new(201).set_body_string("x".repeat(constants::MAX_CREATE_SNAPSHOT_REPLY_BYTES + 1)),
        ResponseTemplate::new(201).set_body_string("not json"),
        ResponseTemplate::new(201).set_body_json(json!({"snapshotID": "owned-1:default", "names": ["named"]})),
        ResponseTemplate::new(201).set_body_json(json!({"snapshotID": "owned-1", "names": []})),
        ResponseTemplate::new(201).set_body_json(json!({"snapshotID": "../escape:default", "names": []})),
    ] {
        let server = MockServer::start().await;
        capture(&server, reply).await;
        assert!(matches!(
            api(&server)?.create_snapshot("source").await,
            Err(ApiError::Transport(_))
        ));
    }
    Ok(())
}

#[tokio::test]
async fn snapshot_absence_is_a_missing_template() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/templates/gone"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/templates/here"))
        .respond_with(ResponseTemplate::new(200).set_body_json(valid_template()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/templates/broken"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let api = api(&server)?;
    assert!(
        api.snapshot_is_absent(&SnapshotTemplateId::try_from("gone".to_owned())?)
            .await?
    );
    assert!(
        !api.snapshot_is_absent(&SnapshotTemplateId::try_from("here".to_owned())?)
            .await?
    );
    assert!(
        api.snapshot_is_absent(&SnapshotTemplateId::try_from("broken".to_owned())?)
            .await
            .is_err()
    );
    Ok(())
}

#[test]
fn config_debug_never_shows_the_key() -> TestResult {
    let config = E2bConfig::new(Url::parse("https://api.e2b.app")?, DEFAULT_DOMAIN, "super-secret-value")?;
    assert!(!format!("{config:?}").contains("super-secret-value"));
    Ok(())
}

#[test]
fn config_refuses_unsafe_endpoints_domains_and_keys() -> TestResult {
    let remote_http = E2bConfig::new(Url::parse("http://example.com")?, "e2b.app", "k");
    assert_eq!(remote_http.err(), Some(E2bConfigError::InsecureEndpoint));
    assert!(E2bConfig::new(Url::parse("http://127.0.0.1:9")?, "e2b.app", "k").is_ok());
    for decorated in [
        "https://user:pw@api.e2b.app",
        "https://api.e2b.app?x=1",
        "https://api.e2b.app#x",
    ] {
        let config = E2bConfig::new(Url::parse(decorated)?, "e2b.app", "k");
        assert_eq!(config.err(), Some(E2bConfigError::DecoratedEndpoint));
    }
    assert_eq!(
        E2bConfig::new(Url::parse("https://api.e2b.app")?, "e2b.app", "").err(),
        Some(E2bConfigError::EmptyApiKey)
    );
    assert_eq!(
        E2bConfig::new(Url::parse("https://api.e2b.app")?, "e2b.app", "bad\nkey").err(),
        Some(E2bConfigError::InvalidApiKey)
    );
    for domain in ["a/b", ""] {
        assert_eq!(
            E2bConfig::new(Url::parse("https://api.e2b.app")?, domain, "k").err(),
            Some(E2bConfigError::InvalidDomain)
        );
    }
    Ok(())
}

#[test]
fn an_envd_override_is_validated_like_the_api_endpoint() -> TestResult {
    let config = E2bConfig::new(Url::parse("https://api.e2b.app")?, "e2b.app", "k")?;
    let overridden = config.clone().with_envd_url(Url::parse("http://127.0.0.1:7")?)?;
    assert_eq!(overridden.envd_url, Some(Url::parse("http://127.0.0.1:7")?));
    assert_eq!(
        config.with_envd_url(Url::parse("http://example.com")?).err(),
        Some(E2bConfigError::InsecureEndpoint)
    );
    Ok(())
}

#[test]
fn native_access_tokens_must_be_header_safe_and_stay_hidden() -> TestResult {
    let token: NativeAccessToken = serde_json::from_value(json!("tok"))?;
    assert_eq!(format!("{token:?}"), "<redacted>");
    assert_eq!(token.into_value()?, "tok");
    for bad in ["", "  ", "bad\ntoken"] {
        let token: NativeAccessToken = serde_json::from_value(json!(bad))?;
        assert_eq!(token.into_value(), Err(NativeAccessTokenError));
    }
    Ok(())
}

#[tokio::test]
async fn create_sends_the_key_and_template_and_reads_the_tokens() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/sandboxes"))
        .and(header("X-API-Key", "test-key"))
        .and(body_json(json!({"templateID": "base", "timeout": 600})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "templateID": "base", "sandboxID": "sbx1", "envdVersion": "0.2.0",
            "envdAccessToken": "envd-tok", "trafficAccessToken": "traffic-tok",
            "domain": "e2b.test"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let info = api(&server)?
        .create_sandbox(&NewSandbox {
            template_id: "base".into(),
            timeout: 600,
            ..Default::default()
        })
        .await?;
    assert_eq!(info.sandbox_id, "sbx1");
    assert_eq!(info.template_id, "base");
    assert_eq!(info.envd_access_token.as_deref(), Some("envd-tok"));
    assert_eq!(info.traffic_access_token.as_deref(), Some("traffic-tok"));
    assert_eq!(info.domain.as_deref(), Some("e2b.test"));
    assert!(!format!("{info:?}").contains("envd-tok"));
    Ok(())
}

#[tokio::test]
async fn errors_map_by_status() -> TestResult {
    let server = MockServer::start().await;
    for (status, retry) in [
        (401_u16, None),
        (404, None),
        (409, None),
        (429, Some("7")),
        (400, None),
        (503, None),
    ] {
        let mut reply = ResponseTemplate::new(status).set_body_json(json!({"code": status, "message": "nope"}));
        if let Some(retry) = retry {
            reply = reply.insert_header("Retry-After", retry);
        }
        Mock::given(method("GET"))
            .and(path(format!("/sandboxes/s{status}")))
            .respond_with(reply)
            .mount(&server)
            .await;
    }
    let api = api(&server)?;
    assert!(matches!(api.get_sandbox("s401").await, Err(ApiError::Unauthorized(m)) if m == "nope"));
    assert!(matches!(api.get_sandbox("s404").await, Err(ApiError::NotFound(_))));
    assert!(matches!(api.get_sandbox("s409").await, Err(ApiError::Conflict(_))));
    assert!(matches!(
        api.get_sandbox("s429").await,
        Err(ApiError::RateLimited {
            retry_after_secs: Some(7),
            ..
        })
    ));
    assert!(matches!(
        api.get_sandbox("s400").await,
        Err(ApiError::Rejected { status: 400, .. })
    ));
    assert!(matches!(
        api.get_sandbox("s503").await,
        Err(ApiError::Unavailable { status: 503, .. })
    ));
    Ok(())
}

#[test]
fn error_messages_describe_what_a_caller_can_do() {
    let limited = ApiError::RateLimited {
        retry_after_secs: Some(7),
        message: "slow".to_owned(),
    };
    assert_eq!(limited.to_string(), "E2B rate limit reached, retry after 7s: slow");
    let unhinted = ApiError::RateLimited {
        retry_after_secs: None,
        message: "slow".to_owned(),
    };
    assert_eq!(unhinted.to_string(), "E2B rate limit reached: slow");
}

#[tokio::test]
async fn an_error_without_a_body_message_falls_back_to_the_status_reason() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sandboxes/plain"))
        .respond_with(ResponseTemplate::new(404).set_body_string("not json"))
        .mount(&server)
        .await;
    assert!(matches!(
        api(&server)?.get_sandbox("plain").await,
        Err(ApiError::NotFound(message)) if message == "Not Found"
    ));
    Ok(())
}

#[tokio::test]
async fn a_malformed_success_body_is_a_transport_error() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sandboxes/odd"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;
    assert!(matches!(
        api(&server)?.get_sandbox("odd").await,
        Err(ApiError::Transport(_))
    ));
    Ok(())
}

#[tokio::test]
async fn get_reads_state() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sandboxes/sbx1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"sandboxID": "sbx1", "state": "paused", "cpuCount": 2, "memoryMB": 512})),
        )
        .mount(&server)
        .await;
    let detail = api(&server)?.get_sandbox("sbx1").await?;
    assert_eq!(detail.state, "paused");
    assert_eq!(detail.cpu_count, Some(2));
    assert_eq!(detail.memory_mb, Some(512));
    Ok(())
}

#[tokio::test]
async fn native_access_is_read_from_the_sandbox_resource() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sandboxes/sbx1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sandboxID": "sbx1", "state": "running", "endAt": "2099-01-01T00:00:00Z",
            "envdAccessToken": "tok", "domain": "e2b.test",
            "network": {"allowPublicTraffic": false}
        })))
        .mount(&server)
        .await;
    let access = api(&server)?.get_native_access("sbx1").await?;
    assert_eq!(access.sandbox_id, "sbx1");
    assert_eq!(access.state, "running");
    assert_eq!(access.end_at, "2099-01-01T00:00:00Z");
    assert_eq!(access.domain.as_deref(), Some("e2b.test"));
    assert_eq!(access.traffic_privacy, TrafficPrivacy::Private);
    assert_eq!(access.token.ok_or("token")?.into_value()?, "tok");
    Ok(())
}

#[test]
fn native_traffic_privacy_preserves_public_private_and_unknown() -> TestResult {
    let response = json!({
        "sandboxID": "sbx1", "state": "running", "endAt": "2099-01-01T00:00:00Z"
    });
    let missing: NativeSandboxAccess = serde_json::from_value(response.clone())?;
    assert_eq!(missing.traffic_privacy, TrafficPrivacy::Unknown);
    for (network, expected) in [
        (Value::Null, TrafficPrivacy::Unknown),
        (json!({}), TrafficPrivacy::Unknown),
        (json!({"allowPublicTraffic": null}), TrafficPrivacy::Unknown),
        (json!({"allowPublicTraffic": true}), TrafficPrivacy::Public),
        (json!({"allowPublicTraffic": false}), TrafficPrivacy::Private),
    ] {
        let mut response = response.clone();
        response["network"] = network;
        let access: NativeSandboxAccess = serde_json::from_value(response)?;
        assert_eq!(access.traffic_privacy, expected);
    }
    Ok(())
}

#[test]
fn malformed_native_traffic_privacy_errors_never_echo_provider_values() {
    for network in [
        json!("sensitive-provider-marker"),
        json!(["sensitive-provider-marker"]),
        json!(true),
        json!(42),
        json!({"allowPublicTraffic": "sensitive-provider-marker"}),
        json!({"allowPublicTraffic": ["sensitive-provider-marker"]}),
        json!({"allowPublicTraffic": {"sensitive-provider-marker": true}}),
        json!({"allowPublicTraffic": 42}),
    ] {
        let response = json!({
            "sandboxID": "sbx1", "state": "running", "endAt": "2099-01-01T00:00:00Z",
            "network": network
        });
        let diagnostic = match serde_json::from_value::<NativeSandboxAccess>(response) {
            Ok(_) => String::new(),
            Err(error) => format!("{error:?}"),
        };
        assert!(diagnostic.contains("native traffic privacy"));
        assert!(!diagnostic.contains("sensitive-provider-marker"));
    }
}

#[tokio::test]
async fn delete_is_idempotent() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/sandboxes/gone"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"code": 404, "message": "missing"})))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/sandboxes/here"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/sandboxes/broken"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let api = api(&server)?;
    assert!(api.delete_sandbox("here").await?);
    assert!(!api.delete_sandbox("gone").await?);
    assert!(api.delete_sandbox("broken").await.is_err());
    Ok(())
}

#[tokio::test]
async fn pause_keeps_memory_and_connect_resumes_with_a_timeout() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sandboxes/sbx1/pause"))
        .and(body_json(json!({"memory": true})))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v2/sandboxes/sbx1/connect"))
        .and(body_json(json!({"timeout": 120})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "templateID": "base", "sandboxID": "sbx1", "envdAccessToken": "fresh"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let api = api(&server)?;
    api.pause_sandbox("sbx1").await?;
    let info = api.connect_sandbox("sbx1", 120).await?;
    assert_eq!(info.envd_access_token.as_deref(), Some("fresh"));
    assert_eq!(api.timeout(), Duration::from_secs(5));
    assert_eq!(api.config().domain, DEFAULT_DOMAIN);
    Ok(())
}

#[tokio::test]
async fn a_second_pause_is_a_conflict() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sandboxes/sbx1/pause"))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({"code": 409, "message": "already paused"})))
        .mount(&server)
        .await;
    assert!(matches!(
        api(&server)?.pause_sandbox("sbx1").await,
        Err(ApiError::Conflict(_))
    ));
    Ok(())
}

#[tokio::test]
async fn ids_that_could_escape_the_path_are_refused() -> TestResult {
    let server = MockServer::start().await;
    let api = api(&server)?;
    let too_long = "a".repeat(constants::MAX_ID_BYTES + 1);
    for id in ["../x", "", too_long.as_str()] {
        assert!(matches!(
            api.get_sandbox(id).await,
            Err(ApiError::Rejected { status: 400, .. })
        ));
    }
    Ok(())
}

#[tokio::test]
async fn an_unreachable_endpoint_is_a_transport_error() -> TestResult {
    let config = E2bConfig::new(Url::parse("http://127.0.0.1:1")?, "e2b.app", "k")?;
    let api = E2bApi::new(config, Duration::from_secs(2))?;
    assert!(matches!(api.get_sandbox("a").await, Err(ApiError::Transport(_))));
    Ok(())
}

#[test]
fn auto_pause_converts_to_and_from_a_bool() {
    assert_eq!(AutoPause::from(true), AutoPause::Enabled);
    assert_eq!(AutoPause::from(false), AutoPause::Disabled);
    assert!(bool::from(AutoPause::Enabled));
    assert!(!bool::from(AutoPause::Disabled));
}
