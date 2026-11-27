mod provider_support;
mod support;

use std::time::Duration;

use provider_support::*;
use serde_json::{Value, json};
use trogon_sandbox_protocol::Provider;
use trogon_sandbox_vercel_worker::{NewSandbox, VercelApi, VercelApiError, VercelProjectId};

fn sandbox_reply() -> Value {
    json!({
        "sandbox": {"name": "contract-native", "persistent": true},
        "session": {"id": "contract-session", "status": "running"},
        "routes": []
    })
}

fn blank(project: &str) -> Result<NewSandbox, Boxed> {
    Ok(NewSandbox::new(VercelProjectId::new(project)?, 60_000, true))
}

async fn create_on(endpoint: &str, new: NewSandbox) -> TestResult {
    let fake = Fake::start().await?;
    fake.on("POST", endpoint, 201, sandbox_reply());
    let created = fake.server.api()?.create_sandbox(&new).await?;
    assert_eq!(created.sandbox.name, "contract-native");
    assert_eq!(created.session.id, "contract-session");
    assert_eq!(fake.total(), 1);
    let calls = fake.calls("POST", endpoint);
    let call = calls.first().ok_or("no create call")?;
    assert_eq!(call.body, serde_json::to_value(&new)?);
    assert!(call.path.contains("teamId=team_1"));
    Ok(())
}

#[tokio::test]
async fn a_legacy_runtime_keeps_the_v2_creation_endpoint() -> TestResult {
    let mut new = blank("prj_1")?;
    new.runtime = Some("node24".to_owned());
    create_on("/v2/sandboxes", new).await
}

#[tokio::test]
async fn a_managed_image_uses_the_v3_creation_endpoint() -> TestResult {
    let mut new = blank("prj_1")?;
    new.image = Some("vercel/sandbox/node:24".to_owned());
    create_on("/v3/sandboxes", new).await
}

#[tokio::test]
async fn a_native_snapshot_without_a_runtime_uses_the_v3_creation_endpoint() -> TestResult {
    let mut new = blank("prj_1")?;
    new.source = Some(json!({"type": "snapshot", "snapshotId": "snapshot-contract"}));
    create_on("/v3/sandboxes", new).await
}

#[tokio::test]
async fn a_minimal_create_body_carries_only_the_required_parts() -> TestResult {
    assert_eq!(
        serde_json::to_value(blank("prj_1")?)?,
        json!({"projectId": "prj_1", "timeout": 60_000, "persistent": true})
    );
    Ok(())
}

async fn image_is_forwarded(reference: Value, expected: &str) -> TestResult {
    let fake = Fake::start().await?;
    fake.on("POST", "/v3/sandboxes", 201, sandbox_reply());
    let request = request_with(|wire| wire["spec"]["image"]["reference"] = reference)?;
    let lease = fake.provider()?.lease(&request, &lease_id("image-contract")).await?;
    assert_eq!(lease.sandbox_id.as_str(), "contract-native");
    let calls = fake.calls("POST", "/v3/sandboxes");
    let body = &calls.first().ok_or("no create call")?.body;
    assert_eq!(body["image"], expected);
    assert!(body.get("runtime").is_none());
    Ok(())
}

#[tokio::test]
async fn a_project_scoped_custom_repository_preserves_its_tag() -> TestResult {
    image_is_forwarded(
        json!({"registry": "", "repository": "project-image", "tag": "stable"}),
        "project-image:stable",
    )
    .await
}

#[tokio::test]
async fn a_project_scoped_custom_repository_preserves_its_digest() -> TestResult {
    let digest = format!("sha256:{}", "0123456789abcdef".repeat(4));
    image_is_forwarded(
        json!({"registry": "", "repository": "project-image", "digest": digest}),
        &format!("project-image@{digest}"),
    )
    .await
}

#[tokio::test]
async fn a_bare_project_scoped_repository_is_sent_as_named() -> TestResult {
    image_is_forwarded(json!({"registry": "", "repository": "project-image"}), "project-image").await
}

#[tokio::test]
async fn a_fully_qualified_vcr_reference_preserves_its_scope_and_tag() -> TestResult {
    image_is_forwarded(
        json!({
            "registry": "vcr.vercel.com",
            "repository": "fixture-team/fixture-project/project-image",
            "tag": "stable"
        }),
        "vcr.vercel.com/fixture-team/fixture-project/project-image:stable",
    )
    .await
}

#[tokio::test]
async fn a_tagged_image_named_like_a_legacy_runtime_does_not_lose_its_tag() -> TestResult {
    image_is_forwarded(
        json!({"registry": "", "repository": "node24", "tag": "stable"}),
        "node24:stable",
    )
    .await
}

#[tokio::test]
async fn requests_carry_the_bearer_token_and_team_scope() -> TestResult {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = seen.clone();
    let server = Server::start(move |request| {
        recorded
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((request.headers.get("authorization").cloned(), request.path.clone()));
        Reply::json(200, sandbox_reply())
    })
    .await?;
    server.api()?.get_sandbox("sbx-a", false).await?;
    let seen = seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (authorization, path) = seen.first().ok_or("no request")?;
    assert_eq!(authorization.as_deref(), Some("Bearer test-token"));
    assert!(path.starts_with("/v2/sandboxes/sbx-a?"));
    assert!(path.contains("teamId=team_1") && path.contains("projectId=prj_1") && path.contains("resume=false"));
    Ok(())
}

#[tokio::test]
async fn the_client_never_prints_its_token() -> TestResult {
    let fake = Fake::start().await?;
    let api = fake.server.api()?;
    assert!(!format!("{api:?}").contains("test-token"));
    assert_eq!(api.config().team_id().as_str(), "team_1");
    Ok(())
}

#[tokio::test]
async fn a_sandbox_is_fetched_with_or_without_resume() -> TestResult {
    let fake = Fake::start().await?;
    fake.on(
        "GET",
        "/v2/sandboxes/sbx-a",
        200,
        record("sess_1", "https://one.example.test"),
    );
    let api = fake.server.api()?;
    let plain = api.get_sandbox("sbx-a", false).await?;
    assert_eq!(plain.routes.first().map(|route| route.port), Some(3000));
    api.get_sandbox("sbx-a", true).await?;
    let calls = fake.calls("GET", "/v2/sandboxes/sbx-a");
    assert!(calls.first().ok_or("no call")?.path.contains("resume=false"));
    assert!(calls.last().ok_or("no call")?.path.contains("resume=true"));
    Ok(())
}

#[tokio::test]
async fn ids_outside_the_safe_alphabet_never_reach_the_network() -> TestResult {
    let fake = Fake::start().await?;
    let api = fake.server.api()?;
    for bad in ["", "a/b", "../x", "a b", &"a".repeat(129)] {
        assert!(matches!(
            api.get_sandbox(bad, false).await,
            Err(VercelApiError::Rejected { status: 400, .. })
        ));
        assert!(matches!(
            api.delete_sandbox(bad).await,
            Err(VercelApiError::Rejected { .. })
        ));
        assert!(matches!(
            api.stop_session(bad).await,
            Err(VercelApiError::Rejected { .. })
        ));
        assert!(matches!(
            api.snapshot_session(bad, None).await,
            Err(VercelApiError::Rejected { .. })
        ));
    }
    assert_eq!(fake.total(), 0);
    Ok(())
}

#[tokio::test]
async fn stopping_a_session_posts_an_empty_json_object() -> TestResult {
    let fake = Fake::start().await?;
    fake.on("POST", "/v2/sandboxes/sessions/s-1_a/stop", 200, json!({}));
    fake.server.api()?.stop_session("s-1_a").await?;
    let calls = fake.calls("POST", "/v2/sandboxes/sessions/s-1_a/stop");
    assert_eq!(calls.first().ok_or("no call")?.body, json!({}));
    Ok(())
}

#[tokio::test]
async fn a_snapshot_names_its_expiration_only_when_asked() -> TestResult {
    let fake = Fake::start().await?;
    let route = "/v3/sandboxes/sessions/sess_1/snapshot";
    fake.on(
        "POST",
        route,
        201,
        json!({"snapshot": {"id": "snap_1", "status": "created"}}),
    );
    let api = fake.server.api()?;
    let default = api.snapshot_session("sess_1", None).await?;
    assert_eq!(default.id, "snap_1");
    assert_eq!(default.expires_at, None);
    api.snapshot_session("sess_1", Some(0)).await?;
    let calls = fake.calls("POST", route);
    assert_eq!(calls.first().ok_or("no call")?.body, json!({}));
    assert_eq!(calls.last().ok_or("no call")?.body, json!({"expiration": 0}));
    Ok(())
}

#[tokio::test]
async fn deleting_a_missing_sandbox_reports_false() -> TestResult {
    let fake = Fake::start().await?;
    fake.script(
        "DELETE",
        "/v2/sandboxes/sbx-a",
        vec![
            (200, json!({})),
            (404, json!({"error": {"message": "gone"}})),
            (500, json!({"error": {"message": "boom"}})),
        ],
    );
    let api = fake.server.api()?;
    assert!(api.delete_sandbox("sbx-a").await?);
    assert!(!api.delete_sandbox("sbx-a").await?);
    assert!(matches!(
        api.delete_sandbox("sbx-a").await,
        Err(VercelApiError::Unavailable { status: 500, .. })
    ));
    let projects = fake.calls("DELETE", "/v2/sandboxes/sbx-a");
    assert!(projects.first().ok_or("no call")?.path.contains("projectId=prj_1"));
    Ok(())
}

async fn failure(status: u16, body: &[u8]) -> Result<VercelApiError, Boxed> {
    let body = body.to_vec();
    let server = Server::start(move |_| Reply::Complete(status, body.clone())).await?;
    server
        .api()?
        .get_sandbox("sbx-a", false)
        .await
        .err()
        .ok_or("accepted".into())
}

#[tokio::test]
async fn a_failure_is_typed_by_its_status() -> TestResult {
    let message = br#"{"error": {"code": "x", "message": "nope"}}"#;
    assert_eq!(
        failure(401, message).await?,
        VercelApiError::Unauthorized("nope".to_owned())
    );
    assert_eq!(
        failure(403, message).await?,
        VercelApiError::Unauthorized("nope".to_owned())
    );
    assert_eq!(
        failure(404, message).await?,
        VercelApiError::NotFound("nope".to_owned())
    );
    assert_eq!(
        failure(409, message).await?,
        VercelApiError::Conflict("nope".to_owned())
    );
    assert_eq!(
        failure(429, message).await?,
        VercelApiError::RateLimited {
            retry_after_secs: None,
            message: "nope".to_owned()
        }
    );
    assert_eq!(
        failure(422, message).await?,
        VercelApiError::Rejected {
            status: 422,
            message: "nope".to_owned()
        }
    );
    assert_eq!(
        failure(502, message).await?,
        VercelApiError::Unavailable {
            status: 502,
            message: "nope".to_owned()
        }
    );
    Ok(())
}

#[tokio::test]
async fn an_error_message_comes_from_the_body_or_the_status() -> TestResult {
    let top_level = failure(400, br#"{"message": "flat"}"#).await?;
    assert_eq!(
        top_level,
        VercelApiError::Rejected {
            status: 400,
            message: "flat".to_owned()
        }
    );
    let not_json = failure(404, b"<html>").await?;
    assert_eq!(not_json, VercelApiError::NotFound("Not Found".to_owned()));
    let no_message = failure(400, br#"{"error": {"code": "x"}}"#).await?;
    assert_eq!(
        no_message,
        VercelApiError::Rejected {
            status: 400,
            message: "Bad Request".to_owned()
        }
    );
    let not_text = failure(400, br#"{"message": 5}"#).await?;
    assert_eq!(
        not_text,
        VercelApiError::Rejected {
            status: 400,
            message: "Bad Request".to_owned()
        }
    );
    let long = json!({"message": "m".repeat(1000)}).to_string();
    let VercelApiError::Rejected { message, .. } = failure(400, long.as_bytes()).await? else {
        return Err("wrong error".into());
    };
    assert_eq!(message.chars().count(), 300);
    Ok(())
}

#[tokio::test]
async fn a_rate_limit_reports_its_retry_after() -> TestResult {
    let body = br#"{"error": {"message": "slow down"}}"#.to_vec();
    let server = Server::start(move |_| Reply::Headed(429, ("Retry-After", "7".to_owned()), body.clone())).await?;
    let error = server
        .api()?
        .get_sandbox("sbx-a", false)
        .await
        .err()
        .ok_or("accepted")?;
    assert_eq!(
        error,
        VercelApiError::RateLimited {
            retry_after_secs: Some(7),
            message: "slow down".to_owned()
        }
    );
    let unusable = Server::start(|_| Reply::Headed(429, ("Retry-After", "soon".to_owned()), b"{}".to_vec())).await?;
    let error = unusable
        .api()?
        .get_sandbox("sbx-a", false)
        .await
        .err()
        .ok_or("accepted")?;
    assert!(matches!(
        error,
        VercelApiError::RateLimited {
            retry_after_secs: None,
            ..
        }
    ));
    Ok(())
}

#[tokio::test]
async fn a_request_that_gets_no_http_answer_is_a_transport_failure() -> TestResult {
    let silent = Server::start(|_| Reply::Silent).await?;
    let quick = VercelApi::new(silent.config()?, Duration::from_millis(200))?;
    assert!(matches!(
        quick.get_sandbox("sbx-a", false).await,
        Err(VercelApiError::Transport(_))
    ));

    let gone = Server::start(|_| Reply::Silent).await?;
    let api = gone.api()?;
    gone.shutdown().await;
    assert!(matches!(
        api.get_sandbox("sbx-a", false).await,
        Err(VercelApiError::Transport(_))
    ));
    Ok(())
}

#[tokio::test]
async fn an_answer_that_is_not_the_expected_shape_is_a_transport_failure() -> TestResult {
    let fake = Fake::start().await?;
    fake.on("GET", "/v2/sandboxes/sbx-a", 200, json!({"unexpected": true}));
    fake.on(
        "POST",
        "/v3/sandboxes/sessions/sess_1/snapshot",
        200,
        json!({"snapshot": 5}),
    );
    let api = fake.server.api()?;
    assert!(matches!(
        api.get_sandbox("sbx-a", false).await,
        Err(VercelApiError::Transport(_))
    ));
    assert!(matches!(
        api.snapshot_session("sess_1", None).await,
        Err(VercelApiError::Transport(_))
    ));
    Ok(())
}

#[tokio::test]
async fn a_body_that_dies_midway_is_a_transport_failure() -> TestResult {
    let truncated = Server::start(|_| Reply::Held(b"{\"sandbox\"".to_vec())).await?;
    let quick = VercelApi::new(truncated.config()?, Duration::from_millis(300))?;
    assert!(matches!(
        quick.get_sandbox("sbx-a", false).await,
        Err(VercelApiError::Transport(_))
    ));
    Ok(())
}
