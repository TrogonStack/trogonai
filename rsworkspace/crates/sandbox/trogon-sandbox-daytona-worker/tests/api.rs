mod support;

use std::time::Duration;

use serde_json::json;
use support::*;
use trogon_sandbox_daytona_worker::{
    DaytonaApi, DaytonaApiError, DaytonaApiKey, DaytonaConfig, DaytonaEndpoint, ExecuteRequest, GitCloneRequest,
    NewSandbox, SandboxState,
};

fn sandbox_route(suffix: &str) -> String {
    format!("/sandbox/{SANDBOX}{suffix}")
}

fn box_route(suffix: &str) -> String {
    format!("/box/{SANDBOX}/{suffix}")
}

#[tokio::test]
async fn a_create_carries_the_credentials_and_the_organization() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let mut new = NewSandbox::new("base:1");
    new.auto_stop_interval = Some(15);
    let record = fake.api()?.create_sandbox(&new).await?;
    assert_eq!(record.id, SANDBOX);
    assert_eq!(record.state, Some(SandboxState::Started));
    let calls = fake.calls("POST", "/sandbox");
    let call = calls.first().ok_or("no create call")?;
    assert!(
        call.headers
            .get("authorization")
            .is_some_and(|value| value.starts_with("Bearer key-"))
    );
    assert_eq!(
        call.headers.get("x-daytona-organization-id").map(String::as_str),
        Some("org-1")
    );
    assert_eq!(call.json(), json!({"snapshot": "base:1", "autoStopInterval": 15}));
    Ok(())
}

#[tokio::test]
async fn a_client_without_an_organization_sends_no_scope() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let config = DaytonaConfig::new(fake.endpoint()?, DaytonaApiKey::new(&key())?, None);
    let api = DaytonaApi::new(config, Duration::from_secs(10))?;
    assert!(api.config().organization().is_none());
    api.get_sandbox(SANDBOX).await?;
    let calls = fake.calls("GET", &sandbox_route(""));
    assert!(
        !calls
            .first()
            .ok_or("no call")?
            .headers
            .contains_key("x-daytona-organization-id")
    );
    Ok(())
}

#[tokio::test]
async fn lifecycle_calls_hit_their_routes() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.json("POST", &sandbox_route("/start"), 200, json!({}));
    fake.json("POST", &sandbox_route("/stop"), 200, json!({}));
    let api = fake.api()?;
    api.start_sandbox(SANDBOX).await?;
    api.stop_sandbox(SANDBOX).await?;
    assert!(api.delete_sandbox(SANDBOX).await?);
    let deletes = fake.calls("DELETE", &sandbox_route(""));
    assert_eq!(deletes.first().ok_or("no delete")?.query, "force=true");
    assert_eq!(fake.count("POST", &sandbox_route("/start")), 1);
    assert_eq!(fake.count("POST", &sandbox_route("/stop")), 1);
    Ok(())
}

#[tokio::test]
async fn deleting_a_gone_sandbox_reports_it_and_other_failures_stay_errors() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.json("DELETE", &sandbox_route(""), 404, json!({"message": "gone"}));
    assert!(!fake.api()?.delete_sandbox(SANDBOX).await?);
    fake.json("DELETE", &sandbox_route(""), 500, json!({"message": "down"}));
    assert!(matches!(
        fake.api()?.delete_sandbox(SANDBOX).await,
        Err(DaytonaApiError::Unavailable { status: 500, .. })
    ));
    Ok(())
}

#[tokio::test]
async fn status_codes_map_to_their_errors() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let api = fake.api()?;
    let route = sandbox_route("");
    fake.json("GET", &route, 401, json!({"message": "bad key"}));
    assert!(matches!(
        api.get_sandbox(SANDBOX).await,
        Err(DaytonaApiError::Unauthorized(_))
    ));
    fake.json("GET", &route, 404, json!({}));
    assert!(matches!(
        api.get_sandbox(SANDBOX).await,
        Err(DaytonaApiError::NotFound(_))
    ));
    fake.json("GET", &route, 409, json!({}));
    assert!(matches!(
        api.get_sandbox(SANDBOX).await,
        Err(DaytonaApiError::Conflict(_))
    ));
    fake.script("GET", &route, vec![Reply::RetryAfter(429, 3, b"{}".to_vec())]);
    assert!(matches!(
        api.get_sandbox(SANDBOX).await,
        Err(DaytonaApiError::RateLimited {
            retry_after_secs: Some(3),
            ..
        })
    ));
    fake.json("GET", &route, 400, json!({"message": "bad"}));
    assert!(matches!(
        api.get_sandbox(SANDBOX).await,
        Err(DaytonaApiError::Rejected { status: 400, .. })
    ));
    fake.raw("GET", &route, 200, b"not json");
    assert!(matches!(
        api.get_sandbox(SANDBOX).await,
        Err(DaytonaApiError::Transport(_))
    ));
    Ok(())
}

#[tokio::test]
async fn a_silent_server_times_out_as_a_transport_error() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.script("GET", &sandbox_route(""), vec![Reply::Silent]);
    let api = DaytonaApi::new(fake.config()?, Duration::from_millis(300))?;
    assert!(matches!(
        api.get_sandbox(SANDBOX).await,
        Err(DaytonaApiError::Transport(_))
    ));
    Ok(())
}

#[tokio::test]
async fn an_unreachable_server_is_a_transport_error() -> TestResult {
    let config = DaytonaConfig::new(
        DaytonaEndpoint::parse("http://127.0.0.1:9")?,
        DaytonaApiKey::new(&key())?,
        None,
    );
    let api = DaytonaApi::new(config, Duration::from_secs(5))?;
    assert!(matches!(
        api.get_sandbox(SANDBOX).await,
        Err(DaytonaApiError::Transport(_))
    ));
    Ok(())
}

#[tokio::test]
async fn unsafe_ids_never_reach_the_network() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let api = fake.api()?;
    assert!(api.get_sandbox("a/b").await.is_err());
    assert!(api.start_sandbox("").await.is_err());
    assert!(api.stop_sandbox("../x").await.is_err());
    assert!(api.delete_sandbox("a b").await.is_err());
    assert!(api.port_preview("a?b", 80).await.is_err());
    assert!(api.download("a/b", "/x").await.is_err());
    assert_eq!(fake.total(), 0);
    Ok(())
}

#[tokio::test]
async fn a_port_preview_returns_the_url_and_token() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.json(
        "GET",
        &sandbox_route("/port-preview/3000"),
        200,
        json!({"url": "https://3000-x.proxy.example", "token": "t"}),
    );
    let preview = fake.api()?.port_preview(SANDBOX, 3000).await?;
    assert_eq!(preview.url, "https://3000-x.proxy.example");
    assert_eq!(preview.token.as_deref(), Some("t"));
    Ok(())
}

#[tokio::test]
async fn the_toolbox_proxy_is_looked_up_once_per_sandbox() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.json("GET", &box_route("files/download"), 200, json!({}));
    let api = fake.api()?;
    api.download(SANDBOX, "/a").await?;
    api.download(SANDBOX, "/b").await?;
    assert_eq!(fake.count("GET", &sandbox_route("/toolbox-proxy-url")), 1);
    api.delete_sandbox(SANDBOX).await?;
    api.download(SANDBOX, "/c").await?;
    assert_eq!(fake.count("GET", &sandbox_route("/toolbox-proxy-url")), 2);
    Ok(())
}

#[tokio::test]
async fn a_failed_proxy_lookup_is_an_error() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.json("GET", &sandbox_route("/toolbox-proxy-url"), 500, json!({}));
    assert!(fake.api()?.download(SANDBOX, "/a").await.is_err());
    Ok(())
}

#[tokio::test]
async fn execute_posts_the_command_and_reads_the_reply() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.executes(&[(3, "out".to_owned())]);
    let reply = fake
        .api()?
        .execute(
            SANDBOX,
            &ExecuteRequest {
                command: "echo hi".to_owned(),
                cwd: Some("/workspace".to_owned()),
                timeout: Some(5),
            },
        )
        .await?;
    assert_eq!((reply.exit_code, reply.result.as_str()), (3, "out"));
    let calls = fake.calls("POST", &box_route("process/execute"));
    assert_eq!(
        calls.first().ok_or("no call")?.json(),
        json!({"command": "echo hi", "cwd": "/workspace", "timeout": 5})
    );
    let plain = fake
        .api()?
        .execute(
            SANDBOX,
            &ExecuteRequest {
                command: "true".to_owned(),
                cwd: None,
                timeout: None,
            },
        )
        .await?;
    assert_eq!(plain.exit_code, 3);
    Ok(())
}

#[tokio::test]
async fn files_download_upload_delete_and_permissions() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.raw("GET", &box_route("files/download"), 200, b"content");
    fake.json("POST", &box_route("files/bulk-upload"), 200, json!({}));
    fake.json("DELETE", &box_route("files"), 200, json!({}));
    fake.json("POST", &box_route("files/permissions"), 200, json!({}));
    let api = fake.api()?;
    assert_eq!(api.download(SANDBOX, "/workspace/a").await?.as_ref(), b"content");
    assert_eq!(
        fake.calls("GET", &box_route("files/download"))[0].query,
        "path=%2Fworkspace%2Fa"
    );
    api.upload(SANDBOX, "/workspace/a", b"payload").await?;
    let uploads = fake.calls("POST", &box_route("files/bulk-upload"));
    let upload = uploads.first().ok_or("no upload")?;
    assert!(
        upload
            .headers
            .get("content-type")
            .is_some_and(|v| v.starts_with("multipart/form-data; boundary="))
    );
    assert!(upload.text().contains("payload"));
    api.delete_file(SANDBOX, "/workspace/a").await?;
    fake.json("DELETE", &box_route("files"), 404, json!({}));
    api.delete_file(SANDBOX, "/workspace/a").await?;
    fake.json("DELETE", &box_route("files"), 500, json!({}));
    assert!(api.delete_file(SANDBOX, "/workspace/a").await.is_err());
    api.set_permissions(SANDBOX, "/workspace/a", 0o750).await?;
    assert_eq!(
        fake.calls("POST", &box_route("files/permissions"))[0].query,
        "path=%2Fworkspace%2Fa&mode=750"
    );
    fake.json("GET", &box_route("files/download"), 404, json!({}));
    assert!(matches!(
        api.download(SANDBOX, "/x").await,
        Err(DaytonaApiError::NotFound(_))
    ));
    Ok(())
}

#[tokio::test]
async fn folders_and_clones_use_the_toolbox() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.json("POST", &box_route("git/clone"), 200, json!({}));
    let api = fake.api()?;
    api.create_folder(SANDBOX, "/workspace", 0o755).await?;
    assert_eq!(
        fake.calls("POST", &box_route("files/folder"))[0].query,
        "path=%2Fworkspace&mode=755"
    );
    api.git_clone(
        SANDBOX,
        &GitCloneRequest {
            url: "https://example.com/r.git".to_owned(),
            path: "/workspace".to_owned(),
            commit_id: Some("abcdef0".to_owned()),
            username: None,
            password: None,
        },
    )
    .await?;
    assert_eq!(
        fake.calls("POST", &box_route("git/clone"))[0].json(),
        json!({"url": "https://example.com/r.git", "path": "/workspace", "commit_id": "abcdef0"})
    );
    Ok(())
}

#[tokio::test]
async fn the_proxy_cache_is_bounded() -> TestResult {
    let fake = Fake::start().await?;
    let api = fake.api()?;
    let total = 1025;
    for index in 0..total {
        fake.json(
            "GET",
            &format!("/sandbox/s{index}/toolbox-proxy-url"),
            200,
            json!({"url": format!("{}/box", fake.url)}),
        );
        fake.json("GET", &format!("/box/s{index}/files/download"), 200, json!({}));
    }
    for index in 0..total {
        api.download(&format!("s{index}"), "/a").await?;
    }
    // The cache emptied at its limit, so the first sandbox is looked up again.
    api.download("s0", "/a").await?;
    assert_eq!(fake.count("GET", "/sandbox/s0/toolbox-proxy-url"), 2);
    Ok(())
}

#[test]
fn a_key_and_an_endpoint_describe_themselves_safely() -> TestResult {
    let secret = key();
    let parsed: DaytonaApiKey = secret.parse()?;
    let shown = format!("{parsed:?}");
    assert!(!shown.contains(&secret), "{shown}");
    assert!(DaytonaEndpoint::parse("https://user:pw@example.com/api").is_err());
    assert!(DaytonaEndpoint::parse("https://example.com/api?x=1").is_err());
    Ok(())
}
