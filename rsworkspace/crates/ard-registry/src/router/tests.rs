use std::sync::Arc;

use ard_catalog::{CatalogEntryWire, CatalogManifest, CatalogManifestWire, SPEC_VERSION};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header::CONTENT_TYPE};
use tower::ServiceExt;

use super::router;
use crate::registry::Registry;
use crate::registry_config::RegistryConfig;

fn sample_registry() -> Arc<Registry> {
    let manifest: CatalogManifest = CatalogManifestWire {
        spec_version: SPEC_VERSION.to_owned(),
        host: None,
        entries: vec![CatalogEntryWire {
            identifier: "urn:air:example.com:agent:assistant".to_owned(),
            display_name: "Assistant".to_owned(),
            media_type: "application/a2a-agent-card+json".to_owned(),
            url: Some("https://example.com/card.json".to_owned()),
            data: None,
            description: Some("Helpful agent".to_owned()),
            representative_queries: Some(vec!["help me".to_owned(), "answer questions".to_owned()]),
            tags: Some(vec!["demo".to_owned()]),
            capabilities: Some(vec!["chat".to_owned()]),
            version: None,
            updated_at: None,
            metadata: None,
            trust_manifest: None,
        }],
    }
    .try_into()
    .unwrap();

    Arc::new(Registry::new(RegistryConfig::new(
        "https://registry.example.com",
        manifest,
        vec![],
    )))
}

fn post_json(uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

#[tokio::test]
async fn serves_manifest() {
    let app = router(sample_registry());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/.well-known/ai-catalog.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["specVersion"], "1.0");
    assert_eq!(json["entries"].as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn search_returns_results() {
    let app = router(sample_registry());
    let response = app
        .oneshot(post_json(
            "/search",
            r#"{"query":{"text":"assistant"},"federation":"none"}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["results"].as_array().map(Vec::len), Some(1));
    assert!(json["results"][0]["score"].as_u64().is_some());
    assert_eq!(json["results"][0]["identifier"], "urn:air:example.com:agent:assistant");
}

#[tokio::test]
async fn search_rejects_empty_query() {
    let app = router(sample_registry());
    let response = app
        .oneshot(post_json("/search", r#"{"query":{"text":"   "}}"#))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn agents_lists_entries() {
    let app = router(sample_registry());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/agents?pageSize=10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["items"].as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn explore_returns_facets() {
    let app = router(sample_registry());
    let response = app
        .oneshot(post_json(
            "/explore",
            r#"{"query":{"text":"assistant"},"resultType":{"facets":[{"field":"type"},{"field":"tags"}]}}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["resultType"], "facets");
    assert!(json["facets"]["type"]["buckets"].is_array());
    assert!(json["facets"]["tags"]["buckets"].is_array());
}

#[tokio::test]
async fn search_includes_configured_referrals() {
    let referral: ard_catalog::CatalogEntry = CatalogEntryWire {
        identifier: "urn:air:peer.example:registry:main".to_owned(),
        display_name: "Peer Registry".to_owned(),
        media_type: "application/ai-registry+json".to_owned(),
        url: Some("https://peer.example/registry".to_owned()),
        data: None,
        description: Some("Peer registry".to_owned()),
        representative_queries: Some(vec!["discover agents".to_owned(), "search catalog".to_owned()]),
        tags: None,
        capabilities: None,
        version: None,
        updated_at: None,
        metadata: None,
        trust_manifest: None,
    }
    .try_into()
    .unwrap();

    let manifest: CatalogManifest = CatalogManifestWire {
        spec_version: SPEC_VERSION.to_owned(),
        host: None,
        entries: vec![],
    }
    .try_into()
    .unwrap();

    let registry = Arc::new(Registry::new(RegistryConfig::new(
        "https://registry.example.com",
        manifest,
        vec![referral],
    )));
    let app = router(registry);
    let response = app
        .oneshot(post_json(
            "/search",
            r#"{"query":{"text":"discover"},"federation":"referrals"}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["referrals"].as_array().map(Vec::len), Some(1));
}
