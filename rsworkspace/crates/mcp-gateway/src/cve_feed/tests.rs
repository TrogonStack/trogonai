use std::time::Duration;

use trogon_std::time::MockClock;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::{PackageEcosystem, VulnerabilitySeverity};

fn coordinate() -> PackageCoordinate {
    PackageCoordinate::new("mcp-server-sqlite", "0.3.1", PackageEcosystem::PyPi).expect("valid coordinate")
}

fn gate_for(server: &MockServer, clock: MockClock) -> OsvCveFeedGate<MockClock> {
    OsvCveFeedGate::new(reqwest::Client::new(), clock).with_api_url(format!("{}/v1/query", server.uri()))
}

#[tokio::test]
async fn vulnerable_package_denies() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "vulns": [{
                "id": "GHSA-xxxx-yyyy-zzzz",
                "aliases": ["CVE-2024-99999"],
                "summary": "remote code execution",
                "severity": [{"score": "9.8"}]
            }]
        })))
        .mount(&server)
        .await;

    let gate = gate_for(&server, MockClock::new());
    let verdict = gate.check(&coordinate()).await;

    match verdict {
        FeedVerdict::DenyVulnerable(records) => {
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].cve_id().as_str(), "CVE-2024-99999");
            assert_eq!(records[0].severity(), VulnerabilitySeverity::Critical);
        }
        other => panic!("expected DenyVulnerable, got {other:?}"),
    }
}

#[tokio::test]
async fn clean_package_allows() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"vulns": []})))
        .mount(&server)
        .await;

    let gate = gate_for(&server, MockClock::new());
    let verdict = gate.check(&coordinate()).await;

    assert_eq!(verdict, FeedVerdict::Allow);
}

#[tokio::test]
async fn unreachable_feed_denies_fail_closed() {
    // No mock mounted; the server rejects the connection, and the gate
    // must translate that into a deny, never a silent allow.
    let server = MockServer::start().await;
    let unreachable_url = format!("{}/does-not-exist", server.uri());
    drop(server);

    let gate = OsvCveFeedGate::new(reqwest::Client::new(), MockClock::new()).with_api_url(unreachable_url);
    let verdict = gate.check(&coordinate()).await;

    match verdict {
        FeedVerdict::DenyUnknown(DenyUnknownReason::FeedUnreachable(_)) => {}
        other => panic!("expected DenyUnknown(FeedUnreachable), got {other:?}"),
    }
}

#[tokio::test]
async fn unreachable_feed_allows_when_explicitly_fail_open() {
    let server = MockServer::start().await;
    let unreachable_url = format!("{}/does-not-exist", server.uri());
    drop(server);

    let gate = OsvCveFeedGate::new(reqwest::Client::new(), MockClock::new())
        .with_api_url(unreachable_url)
        .with_unreachable_policy(CveFeedUnreachablePolicy::Allow);
    let verdict = gate.check(&coordinate()).await;

    assert_eq!(verdict, FeedVerdict::Allow);
}

#[tokio::test]
async fn cache_hit_avoids_second_http_call() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"vulns": []})))
        .expect(1)
        .mount(&server)
        .await;

    let clock = MockClock::new();
    let gate = gate_for(&server, clock.clone());

    let first = gate.check(&coordinate()).await;
    let second = gate.check(&coordinate()).await;

    assert_eq!(first, FeedVerdict::Allow);
    assert_eq!(second, FeedVerdict::Allow);
    // wiremock's `.expect(1)` is verified on drop; reaching here without a
    // panic on server teardown means only one HTTP call was made.
}

#[tokio::test]
async fn cache_expires_after_one_hour_via_injected_clock() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"vulns": []})))
        .expect(2)
        .mount(&server)
        .await;

    let clock = MockClock::new();
    let gate = gate_for(&server, clock.clone());

    let first = gate.check(&coordinate()).await;
    clock.advance(Duration::from_secs(3601));
    let second = gate.check(&coordinate()).await;

    assert_eq!(first, FeedVerdict::Allow);
    assert_eq!(second, FeedVerdict::Allow);
}

#[tokio::test]
async fn cache_still_fresh_just_under_one_hour() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"vulns": []})))
        .expect(1)
        .mount(&server)
        .await;

    let clock = MockClock::new();
    let gate = gate_for(&server, clock.clone());

    let _first = gate.check(&coordinate()).await;
    clock.advance(Duration::from_secs(3599));
    let second = gate.check(&coordinate()).await;

    assert_eq!(second, FeedVerdict::Allow);
}

#[tokio::test]
async fn malformed_osv_response_denies() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/query"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("not json", "application/json"))
        .mount(&server)
        .await;

    let gate = gate_for(&server, MockClock::new());
    let verdict = gate.check(&coordinate()).await;

    match verdict {
        FeedVerdict::DenyUnknown(DenyUnknownReason::MalformedResponse(_)) => {}
        other => panic!("expected DenyUnknown(MalformedResponse), got {other:?}"),
    }
}

#[tokio::test]
async fn server_error_status_denies_fail_closed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/query"))
        .respond_with(ResponseTemplate::new(503).set_body_raw("service unavailable", "text/plain"))
        .mount(&server)
        .await;

    let gate = gate_for(&server, MockClock::new());
    let verdict = gate.check(&coordinate()).await;

    match verdict {
        FeedVerdict::DenyUnknown(DenyUnknownReason::FeedUnreachable(_)) => {}
        other => panic!("expected DenyUnknown(FeedUnreachable), got {other:?}"),
    }
}
