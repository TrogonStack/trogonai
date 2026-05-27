use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::http::{HeaderMap, HeaderValue};
use axum::{Json, Router, routing::get};
use tokio::sync::{RwLock, watch};
use tracing::warn;

use crate::jwks::Jwks;

const WELL_KNOWN_PATH: &str = "/.well-known/jwks.json";
const CACHE_CONTROL: &str = "public, max-age=300";

#[derive(Clone)]
struct AppState {
    jwks: Arc<RwLock<Jwks>>,
}

pub async fn run_https_publisher(
    listen: SocketAddr,
    mut watch_rx: watch::Receiver<Jwks>,
    tls_cert: Option<PathBuf>,
    tls_key: Option<PathBuf>,
) -> Result<(), std::io::Error> {
    let state = AppState {
        jwks: Arc::new(RwLock::new(watch_rx.borrow().clone())),
    };
    let cache = Arc::clone(&state.jwks);
    tokio::spawn(async move {
        while watch_rx.changed().await.is_ok() {
            let jwks = watch_rx.borrow().clone();
            *cache.write().await = jwks;
        }
    });

    let app = Router::new()
        .route(WELL_KNOWN_PATH, get(jwks_handler))
        .with_state(state);

    match (tls_cert, tls_key) {
        (Some(cert_path), Some(key_path)) => {
            let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert_path, key_path).await?;
            axum_server::bind_rustls(listen, config)
                .serve(app.into_make_service())
                .await?;
        }
        _ => {
            if listen.ip().is_loopback() {
                warn!(
                    listen = %listen,
                    "HTTPS publisher serving plaintext JWKS on loopback (dev mode); provide --tls-cert and --tls-key for TLS"
                );
            } else {
                warn!(
                    listen = %listen,
                    "HTTPS publisher serving plaintext JWKS without TLS; provide --tls-cert and --tls-key for production"
                );
            }
            let listener = tokio::net::TcpListener::bind(listen).await?;
            axum::serve(listener, app).await?;
        }
    }

    Ok(())
}

async fn jwks_handler(state: axum::extract::State<AppState>) -> (HeaderMap, Json<Jwks>) {
    let jwks = state.jwks.read().await.clone();
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static(CACHE_CONTROL),
    );
    (headers, Json(jwks))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use jsonwebtoken::jwk::{AlgorithmParameters, CommonParameters, Jwk, PublicKeyUse, RSAKeyParameters, RSAKeyType};
    use tower::ServiceExt;

    #[tokio::test]
    async fn serves_current_jwks_from_cache() {
        let jwks = Jwks {
            keys: vec![Jwk {
                common: CommonParameters {
                    public_key_use: Some(PublicKeyUse::Signature),
                    key_id: Some("http-kid".into()),
                    ..Default::default()
                },
                algorithm: AlgorithmParameters::RSA(RSAKeyParameters {
                    key_type: RSAKeyType::RSA,
                    n: "abc".into(),
                    e: "AQAB".into(),
                }),
            }],
        };
        let state = AppState {
            jwks: Arc::new(RwLock::new(jwks.clone())),
        };
        let app = Router::new()
            .route(WELL_KNOWN_PATH, get(jwks_handler))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri(WELL_KNOWN_PATH)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let served: Jwks = serde_json::from_slice(&body).expect("json");
        assert_eq!(served, jwks);
    }
}
