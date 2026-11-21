use super::*;
use crate::IssuerInput;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn input() -> Result<IssuerInput, serde_json::Error> {
    serde_json::from_value(json!({
        "issuer":"http://127.0.0.1:1", "jwks_uri":"http://127.0.0.1:1/jwks", "audience":"mcp",
        "users":{"user":"local-user"},"clients":{"agent":"local-agent"}
    }))
}

#[test]
fn validates_configuration_without_accepting_unsafe_endpoints_or_mappings() -> TestResult {
    for value in [
        "not a url",
        "ftp://localhost/jwks",
        "https://user:password@example.test/jwks",
        "https://example.test/#fragment",
    ] {
        let mut config = input()?;
        config.jwks_uri = value.to_owned();
        assert!(matches!(Issuer::try_from(config), Err(AuthError::Configuration)));
    }
    for field in [
        "issuer",
        "user_claim",
        "client_claim",
        "scope_claim",
        "required_scopes",
        "users",
        "clients",
    ] {
        let mut value = serde_json::to_value(json!({
            "issuer":"https://example.test", "jwks_uri":"https://example.test/jwks", "audience":"mcp",
            "users":{"user":"local-user"},"clients":{"agent":"local-agent"}
        }))?;
        value[field] = match field {
            "issuer" => json!("https://example.test?query"),
            "required_scopes" => json!(["two scopes"]),
            "users" | "clients" => json!({}),
            _ => json!(""),
        };
        assert!(matches!(
            Issuer::try_from(serde_json::from_value::<IssuerInput>(value)?),
            Err(AuthError::Configuration)
        ));
    }
    for mapping in [json!({"":"valid"}), json!({"user":"invalid@local"})] {
        let mut config = input()?;
        config.users = serde_json::from_value(mapping)?;
        assert!(matches!(Issuer::try_from(config), Err(AuthError::Configuration)));
    }
    Ok(())
}

#[tokio::test]
async fn refuses_empty_configuration_and_unreachable_key_source() -> TestResult {
    assert!(matches!(
        Authenticator::new(AuthenticationConfigInput { issuers: vec![] }).await,
        Err(AuthError::Configuration)
    ));
    assert!(matches!(
        Authenticator::new(AuthenticationConfigInput {
            issuers: vec![input()?]
        })
        .await,
        Err(AuthError::Unavailable)
    ));
    Ok(())
}

#[test]
fn bounds_delegation_and_accepts_array_scopes() -> TestResult {
    let mut config = Issuer::try_from(input()?)?;
    config.required_scopes = vec!["read".to_owned()];
    let mut claims = json!({"sub":"user","client_id":"agent","scope":["read"]});
    assert!(principal(&config, &claims).is_ok());
    claims["scope"] = json!([1]);
    assert!(matches!(principal(&config, &claims), Err(AuthError::Unauthenticated)));
    claims["scope"] = json!(null);
    assert!(matches!(principal(&config, &claims), Err(AuthError::Unauthenticated)));
    claims.as_object_mut().ok_or("claims")?.remove("scope");
    assert!(matches!(principal(&config, &claims), Err(AuthError::Unauthenticated)));
    config.required_scopes.clear();
    assert!(principal(&config, &claims).is_ok());
    let mut act = json!({"sub":"actor-8"});
    for index in (0..8).rev() {
        act = json!({"sub":format!("actor-{index}"),"act":act});
    }
    for index in 0..9 {
        config.clients.insert(
            format!("actor-{index}"),
            crate::ObjectId::new(format!("actor-{index}"))?,
        );
    }
    claims["act"] = act;
    assert!(matches!(principal(&config, &claims), Err(AuthError::Unauthenticated)));
    claims["act"] = json!(null);
    assert!(matches!(principal(&config, &claims), Err(AuthError::Unauthenticated)));
    Ok(())
}

#[tokio::test]
async fn expires_cached_keys_and_throttles_failed_refresh() -> TestResult {
    let config = Issuer::try_from(input()?)?;
    let state = IssuerState {
        config,
        keys: Mutex::new(KeyCache {
            keys: JwkSet { keys: vec![] },
            fetched: Instant::now() - Duration::from_secs(400),
            attempted: Some(Instant::now()),
        }),
    };
    let auth = Authenticator {
        issuers: BTreeMap::new(),
        client: reqwest::Client::new(),
    };
    assert!(matches!(
        auth.key(&state, "missing", Algorithm::ES256).await,
        Err(AuthError::Unavailable)
    ));
    state.keys.lock().await.attempted = None;
    assert!(matches!(
        auth.key(&state, "missing", Algorithm::ES256).await,
        Err(AuthError::Unavailable)
    ));
    assert!(state.keys.lock().await.attempted.is_some());
    Ok(())
}

async fn source(response: Vec<u8>) -> Result<(Issuer, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let mut config = input()?;
    config.jwks_uri = format!("http://{}/jwks", listener.local_addr()?);
    let task = tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut bytes = [0; 4096];
            let _ = socket.read(&mut bytes).await;
            let _ = socket.write_all(&response).await;
        }
    });
    Ok((Issuer::try_from(config)?, task))
}

#[tokio::test]
async fn rejects_failed_oversized_truncated_and_malformed_key_responses() -> TestResult {
    let oversized = "a".repeat(crate::constants::MAX_JWKS_BYTES + 1);
    let responses = [
        b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\n\r\n".to_vec(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 999999\r\n\r\n".to_vec(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n{".to_vec(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\n{".to_vec(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n{\"keys\":[]}".to_vec(),
        format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
            oversized.len(),
            oversized
        )
        .into_bytes(),
    ];
    for response in responses {
        let (issuer, task) = source(response).await?;
        assert!(matches!(
            fetch_keys(&reqwest::Client::new(), &issuer).await,
            Err(AuthError::Unavailable)
        ));
        task.await?;
    }
    Ok(())
}
