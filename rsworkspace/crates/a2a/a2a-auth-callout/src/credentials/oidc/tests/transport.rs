use super::*;

async fn issuer_server(status: u16, issuer_suffix: &str, padding: usize) -> wiremock::MockServer {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::path("/.well-known/openid-configuration"))
        .respond_with(
            wiremock::ResponseTemplate::new(status).set_body_json(serde_json::json!({
                "issuer": format!("{}{issuer_suffix}", server.uri()),
                "jwks_uri": format!("{}/jwks", server.uri()),
                "padding": "x".repeat(padding),
            })),
        )
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn discovery_rejects_unsuccessful_status_even_with_valid_body() {
    for status in [302, 503] {
        let server = issuer_server(status, "", 0).await;
        let issuer = OidcIssuerUrl::loopback_for_test(server.uri());
        assert!(matches!(
            JwksOidcVerifier::discover(issuer, vec!["aud".into()]).await,
            Err(AuthCalloutError::CredentialVerification(CredentialError::OidcDiscovery(
                OidcDiscoveryError::Status(code)
            ))) if code.as_u16() == status
        ));
    }
}

#[tokio::test]
async fn discovery_requires_exact_issuer_identity() {
    let server = issuer_server(200, "/", 0).await;
    let issuer = OidcIssuerUrl::loopback_for_test(server.uri());
    assert!(JwksOidcVerifier::discover(issuer, vec!["aud".into()]).await.is_err());
}

#[tokio::test]
async fn discovery_rejects_oversized_document() {
    let server = issuer_server(200, "", 300_000).await;
    let issuer = OidcIssuerUrl::loopback_for_test(server.uri());
    assert!(JwksOidcVerifier::discover(issuer, vec!["aud".into()]).await.is_err());
}

#[tokio::test]
async fn jwks_rejects_unsuccessful_status_even_with_valid_keys() {
    let server = issuer_server(200, "", 0).await;
    wiremock::Mock::given(wiremock::matchers::path("/jwks"))
        .respond_with(wiremock::ResponseTemplate::new(503).set_body_json(serde_json::json!({"keys": []})))
        .mount(&server)
        .await;
    let issuer = OidcIssuerUrl::loopback_for_test(server.uri());
    let verifier = JwksOidcVerifier::discover(issuer, vec!["aud".into()]).await.unwrap();
    assert!(verifier.fetch_jwks().await.is_err());
}

#[tokio::test]
async fn jwks_rejects_oversized_document() {
    let server = issuer_server(200, "", 0).await;
    wiremock::Mock::given(wiremock::matchers::path("/jwks"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "keys": [], "padding": "x".repeat(300_000),
        })))
        .mount(&server)
        .await;
    let issuer = OidcIssuerUrl::loopback_for_test(server.uri());
    let verifier = JwksOidcVerifier::discover(issuer, vec!["aud".into()]).await.unwrap();
    assert!(verifier.fetch_jwks().await.is_err());
}

#[tokio::test]
async fn chunked_jwks_cannot_bypass_the_body_limit() {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let serving = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut head = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !head.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).unwrap();
            assert_ne!(count, 0);
            head.extend_from_slice(&buffer[..count]);
            assert!(head.len() < 8192);
        }
        let body = serde_json::json!({"keys": [], "padding": "x".repeat(300_000)}).to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes());
    });
    let result = super::super::oidc_discovery_error::fetch_document::<JwkSet>(
        &reqwest::Client::new(),
        &format!("http://{address}/jwks"),
    )
    .await;
    serving.join().unwrap();
    assert!(matches!(result, Err(OidcDiscoveryError::TooLarge)));
}
