mod support;

use serde_json::json;
use support::{RejectionMessage, TestResult, artifact, limits, no_authorization, sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn manifest_server(
    blob: &[u8],
) -> Result<(MockServer, trogon_sandbox_contract::OciArtifactRef), Box<dyn std::error::Error>> {
    let server = MockServer::start().await;
    let (target, raw) = artifact(&server.address().to_string(), blob)?;
    Mock::given(method("GET"))
        .and(path(format!("/v2/test/manifests/{}", target.digest)))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(raw))
        .mount(&server)
        .await;
    Ok((server, target))
}

async fn pull_fails(
    server: &MockServer,
    target: &trogon_sandbox_contract::OciArtifactRef,
) -> Result<String, Box<dyn std::error::Error>> {
    let client = support::client_for(&server.address().to_string())?;
    Ok(client
        .pull_artifact_bounded(target, limits())
        .await
        .err()
        .ok_or("pull must fail")?
        .rejection_message()?
        .to_owned())
}

#[tokio::test]
async fn unusable_authentication_challenges_are_refused() -> TestResult {
    let long_service = "s".repeat(17 * 1024);
    for case in 0..6 {
        let (server, target) = manifest_server(b"blob").await?;
        let registry = server.address().to_string();
        let realm = format!("http://{registry}/token");
        let challenge = match case {
            0 => format!("Bearer realm=\"{realm}\",realm=\"{realm}\""),
            1 => format!("Bearer realm=\"{realm}\",service=\"a\",service=\"b\""),
            2 => "Bearer service=\"fixture\"".to_string(),
            3 => "Negotiate".to_string(),
            4 => format!("Bearer realm=\"{realm}?scope=other\""),
            _ => format!("Bearer realm=\"{realm}\",service=\"{long_service}\""),
        };
        server.reset().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401).insert_header("www-authenticate", challenge.as_str()))
            .mount(&server)
            .await;
        assert!(!pull_fails(&server, &target).await?.is_empty(), "case {case}");
    }
    Ok(())
}

#[tokio::test]
async fn token_responses_are_checked() -> TestResult {
    let cases = [
        (json!({"token": "a", "access_token": "a"}), true),
        (json!({"token": "a", "access_token": "b"}), false),
        (json!({}), false),
        (json!({"token": ""}), false),
        (json!({"token": "bad\nvalue"}), false),
    ];
    for (body, accepted) in cases {
        let (server, target) = manifest_server(b"blob").await?;
        let registry = server.address().to_string();
        server.reset().await;
        let (_, raw) = artifact(&registry, b"blob")?;
        let manifest_path = format!("/v2/test/manifests/{}", target.digest);
        Mock::given(method("GET"))
            .and(path(&manifest_path))
            .and(header("authorization", "Bearer a"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(raw))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v2/test/blobs/".to_string() + &sha256(b"blob")))
            .and(header("authorization", "Bearer a"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"blob".to_vec()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(&manifest_path))
            .and(no_authorization)
            .respond_with(ResponseTemplate::new(401).insert_header(
                "www-authenticate",
                format!("Bearer realm=\"http://{registry}/token?extra=1\",service=\"fixture\"").as_str(),
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body.clone()))
            .mount(&server)
            .await;
        let client = support::client_for(&registry)?;
        let result = client.pull_artifact_bounded(&target, limits()).await;
        assert_eq!(result.is_ok(), accepted, "{body}: {result:?}");
    }
    Ok(())
}

#[tokio::test]
async fn token_endpoint_failures_are_refused() -> TestResult {
    let (server, target) = manifest_server(b"blob").await?;
    let registry = server.address().to_string();
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401).insert_header(
            "www-authenticate",
            format!("Bearer realm=\"http://{registry}/token\"").as_str(),
        ))
        .mount(&server)
        .await;
    assert!(pull_fails(&server, &target).await?.contains("HTTP 500"));
    Ok(())
}

#[tokio::test]
async fn blob_redirects_are_checked() -> TestResult {
    let long = "a".repeat(17 * 1024);
    let locations = [
        None,
        Some(long),
        Some("http://user:secret@127.0.0.1/x".to_string()),
        Some("http://downgrade.invalid/x".to_string()),
        Some("\u{e9}".to_string()),
        Some("loop".to_string()),
    ];
    for (case, location) in locations.iter().enumerate() {
        let (server, target) = manifest_server(b"blob").await?;
        let registry = server.address().to_string();
        let blob_path = format!("/v2/test/blobs/{}", sha256(b"blob"));
        let mut redirect = ResponseTemplate::new(302);
        if let Some(location) = location {
            let value = if location == "loop" {
                blob_path.clone()
            } else {
                location.clone()
            };
            redirect = redirect.insert_header("location", value.as_bytes());
        }
        Mock::given(method("GET"))
            .and(path(&blob_path))
            .respond_with(redirect)
            .mount(&server)
            .await;
        let error = pull_fails(&server, &target).await?;
        assert!(!error.contains("secret"), "case {case}: {error}");
        assert!(!error.contains(&registry) || case == 5, "case {case}: {error}");
    }
    Ok(())
}

#[tokio::test]
async fn relative_blob_redirects_are_followed() -> TestResult {
    let (server, target) = manifest_server(b"blob").await?;
    let blob_path = format!("/v2/test/blobs/{}", sha256(b"blob"));
    Mock::given(method("GET"))
        .and(path(&blob_path))
        .respond_with(ResponseTemplate::new(307).insert_header("location", "/storage/blob"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/storage/blob"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"blob".to_vec()))
        .mount(&server)
        .await;
    let client = support::client_for(&server.address().to_string())?;
    let pulled = client.pull_artifact_bounded(&target, limits()).await?;
    assert_eq!(pulled.layers[0].data, b"blob");
    Ok(())
}

#[tokio::test]
async fn blob_length_mismatches_are_refused() -> TestResult {
    let (server, target) = manifest_server(b"blob").await?;
    let blob_path = format!("/v2/test/blobs/{}", sha256(b"blob"));
    Mock::given(method("GET"))
        .and(path(&blob_path))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"blobbb".to_vec()))
        .mount(&server)
        .await;
    assert!(pull_fails(&server, &target).await?.contains("Content-Length"));
    Ok(())
}

#[tokio::test]
async fn oversized_content_length_is_refused_for_manifests() -> TestResult {
    let server = MockServer::start().await;
    let (target, _) = artifact(&server.address().to_string(), b"blob")?;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b' '; 1024 * 1024 + 1]))
        .mount(&server)
        .await;
    assert!(pull_fails(&server, &target).await?.contains("budget"));
    Ok(())
}

#[tokio::test]
async fn chunked_blobs_shorter_than_declared_are_refused() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?.to_string();
    let (target, raw) = artifact(&address, b"blob")?;
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let raw = raw.clone();
            tokio::spawn(async move {
                let mut request = vec![0u8; 4096];
                let read = socket.read(&mut request).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&request[..read]).to_string();
                let response = if head.contains("/blobs/") {
                    b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n2\r\nbl\r\n0\r\n\r\n"
                        .to_vec()
                } else {
                    let mut response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        raw.len()
                    )
                    .into_bytes();
                    response.extend_from_slice(&raw);
                    response
                };
                let _ = socket.write_all(&response).await;
            });
        }
    });
    let client = support::client_for(&address)?;
    let error = client
        .pull_artifact_bounded(&target, limits())
        .await
        .err()
        .ok_or("pull must fail")?;
    assert!(error.rejection_message()?.contains("length differs"), "{error}");
    Ok(())
}
