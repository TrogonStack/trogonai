//! End-to-end integration tests for `trogon-splitio`.
//!
//! These tests exercise the full stack: typed `FeatureFlag` enum → `SplitClient`
//! → `MockEvaluator` HTTP server, without any real Split.io / Harness account.

use std::collections::HashMap;

use serde_json::json;
use trogon_splitio::{
    SplitClient, SplitConfig,
    flags::FeatureFlag,
    mock::MockEvaluator,
};

// ── Shared flag enum (Spacedrive-inspired typed pattern) ──────────────────────

enum AppFlag {
    NewCheckout,
    BetaDashboard,
    ExperimentalSearch,
    ThemeConfig,
}

impl FeatureFlag for AppFlag {
    fn name(&self) -> &'static str {
        match self {
            Self::NewCheckout        => "new_checkout",
            Self::BetaDashboard      => "beta_dashboard",
            Self::ExperimentalSearch => "experimental_search",
            Self::ThemeConfig        => "theme_config",
        }
    }
}

fn client_for(addr: std::net::SocketAddr) -> SplitClient {
    SplitClient::new(SplitConfig {
        evaluator_url: format!("http://{addr}"),
        auth_token: "test-token".to_string(),
    })
}

// ── is_enabled ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn typed_flag_is_enabled_returns_true_when_on() {
    let mock = MockEvaluator::new().with_flag("new_checkout", "on");
    let (addr, _h) = mock.serve().await;
    let client = client_for(addr);

    assert!(client.is_enabled("user-1", &AppFlag::NewCheckout, None).await);
}

#[tokio::test]
async fn typed_flag_is_enabled_returns_false_when_off() {
    let mock = MockEvaluator::new().with_flag("new_checkout", "off");
    let (addr, _h) = mock.serve().await;
    let client = client_for(addr);

    assert!(!client.is_enabled("user-1", &AppFlag::NewCheckout, None).await);
}

#[tokio::test]
async fn typed_flag_is_enabled_returns_false_for_undefined_flag() {
    // Flag not registered → mock returns "control" → is_enabled is false
    let mock = MockEvaluator::new();
    let (addr, _h) = mock.serve().await;
    let client = client_for(addr);

    assert!(!client.is_enabled("user-1", &AppFlag::ExperimentalSearch, None).await);
}

#[tokio::test]
async fn typed_flag_is_enabled_with_attributes() {
    let mock = MockEvaluator::new().with_flag("beta_dashboard", "on");
    let (addr, _h) = mock.serve().await;
    let client = client_for(addr);

    let mut attrs = HashMap::new();
    attrs.insert("plan".to_string(), json!("premium"));

    // Mock ignores attributes but the request must succeed
    assert!(client.is_enabled("user-1", &AppFlag::BetaDashboard, Some(&attrs)).await);
}

// ── all_enabled / any_enabled ─────────────────────────────────────────────────

#[tokio::test]
async fn all_enabled_true_when_all_flags_on() {
    let mock = MockEvaluator::new()
        .with_flag("new_checkout", "on")
        .with_flag("beta_dashboard", "on");
    let (addr, _h) = mock.serve().await;
    let client = client_for(addr);

    let flags: &[&dyn FeatureFlag] = &[&AppFlag::NewCheckout, &AppFlag::BetaDashboard];
    assert!(client.all_enabled("user-1", flags, None).await);
}

#[tokio::test]
async fn all_enabled_false_when_one_flag_off() {
    let mock = MockEvaluator::new()
        .with_flag("new_checkout", "on")
        .with_flag("beta_dashboard", "off");
    let (addr, _h) = mock.serve().await;
    let client = client_for(addr);

    let flags: &[&dyn FeatureFlag] = &[&AppFlag::NewCheckout, &AppFlag::BetaDashboard];
    assert!(!client.all_enabled("user-1", flags, None).await);
}

#[tokio::test]
async fn any_enabled_true_when_at_least_one_on() {
    let mock = MockEvaluator::new()
        .with_flag("new_checkout", "off")
        .with_flag("beta_dashboard", "on");
    let (addr, _h) = mock.serve().await;
    let client = client_for(addr);

    let flags: &[&dyn FeatureFlag] = &[&AppFlag::NewCheckout, &AppFlag::BetaDashboard];
    assert!(client.any_enabled("user-1", flags, None).await);
}

#[tokio::test]
async fn any_enabled_false_when_all_off() {
    let mock = MockEvaluator::new()
        .with_flag("new_checkout", "off")
        .with_flag("beta_dashboard", "off");
    let (addr, _h) = mock.serve().await;
    let client = client_for(addr);

    let flags: &[&dyn FeatureFlag] = &[&AppFlag::NewCheckout, &AppFlag::BetaDashboard];
    assert!(!client.any_enabled("user-1", flags, None).await);
}

// ── treatment_for (raw treatment) ────────────────────────────────────────────

#[tokio::test]
async fn treatment_for_returns_custom_variant() {
    let mock = MockEvaluator::new().with_flag("new_checkout", "blue");
    let (addr, _h) = mock.serve().await;
    let client = client_for(addr);

    let t = client.treatment_for("user-1", &AppFlag::NewCheckout, None).await.unwrap();
    assert_eq!(t, "blue");
}

// ── get_treatments (bulk) ─────────────────────────────────────────────────────

#[tokio::test]
async fn bulk_get_treatments_with_typed_flags() {
    let mock = MockEvaluator::new()
        .with_flag("new_checkout", "on")
        .with_flag("beta_dashboard", "off");
    let (addr, _h) = mock.serve().await;
    let client = client_for(addr);

    let names: &[&str] = &[
        AppFlag::NewCheckout.name(),
        AppFlag::BetaDashboard.name(),
        AppFlag::ExperimentalSearch.name(), // not registered → control
    ];
    let map = client.get_treatments("user-1", names, None).await.unwrap();
    assert_eq!(map[AppFlag::NewCheckout.name()], "on");
    assert_eq!(map[AppFlag::BetaDashboard.name()], "off");
    assert_eq!(map[AppFlag::ExperimentalSearch.name()], "control");
}

// ── get_treatment_with_config ─────────────────────────────────────────────────

#[tokio::test]
async fn treatment_with_config_returns_json_payload() {
    let mock = MockEvaluator::new().with_flag_and_config(
        "theme_config",
        "on",
        json!({"primary": "#ff0000", "dark_mode": true}),
    );
    let (addr, _h) = mock.serve().await;
    let client = client_for(addr);

    let result = client
        .get_treatment_with_config("user-1", AppFlag::ThemeConfig.name(), None)
        .await
        .unwrap();
    assert_eq!(result.treatment, "on");
    let cfg = result.config.unwrap();
    assert_eq!(cfg["primary"], "#ff0000");
    assert_eq!(cfg["dark_mode"], true);
}

#[tokio::test]
async fn treatment_with_config_null_for_plain_flag() {
    let mock = MockEvaluator::new().with_flag("new_checkout", "on");
    let (addr, _h) = mock.serve().await;
    let client = client_for(addr);

    let result = client
        .get_treatment_with_config("user-1", AppFlag::NewCheckout.name(), None)
        .await
        .unwrap();
    assert_eq!(result.treatment, "on");
    assert!(result.config.is_none());
}

// ── get_treatment_or_control (safe fallback) ──────────────────────────────────

#[tokio::test]
async fn treatment_or_control_returns_control_when_evaluator_down() {
    let client = SplitClient::new(SplitConfig {
        evaluator_url: "http://127.0.0.1:1".to_string(),
        auth_token: "tok".to_string(),
    });
    let t = client
        .get_treatment_or_control("user-1", AppFlag::NewCheckout.name(), None)
        .await;
    assert_eq!(t, trogon_splitio::CONTROL);
}

#[tokio::test]
async fn treatment_or_control_returns_treatment_when_ok() {
    let mock = MockEvaluator::new().with_flag("new_checkout", "on");
    let (addr, _h) = mock.serve().await;
    let client = client_for(addr);

    let t = client
        .get_treatment_or_control("user-1", AppFlag::NewCheckout.name(), None)
        .await;
    assert_eq!(t, "on");
}

// ── track ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn track_event_is_recorded_by_mock() {
    let mock = MockEvaluator::new();
    let events = mock.tracked_events.clone();
    let (addr, _h) = mock.serve().await;
    let client = client_for(addr);

    client
        .track("user-1", "user", "checkout_completed", Some(99.0), None)
        .await
        .unwrap();

    let evs = events.lock().unwrap();
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].key, "user-1");
    assert_eq!(evs[0].event_type, "checkout_completed");
    assert_eq!(evs[0].value, Some(99.0));
}

#[tokio::test]
async fn track_event_without_value() {
    let mock = MockEvaluator::new();
    let events = mock.tracked_events.clone();
    let (addr, _h) = mock.serve().await;
    let client = client_for(addr);

    client.track("user-2", "user", "page_view", None, None).await.unwrap();

    let evs = events.lock().unwrap();
    assert_eq!(evs[0].value, None);
}

// ── health check ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn healthcheck_returns_true_for_running_mock() {
    let mock = MockEvaluator::new();
    let (addr, _h) = mock.serve().await;
    let client = client_for(addr);

    assert!(client.is_healthy().await);
}

#[tokio::test]
async fn healthcheck_returns_false_when_evaluator_down() {
    let client = SplitClient::new(SplitConfig {
        evaluator_url: "http://127.0.0.1:1".to_string(),
        auth_token: "tok".to_string(),
    });
    assert!(!client.is_healthy().await);
}

// ── description helper ────────────────────────────────────────────────────────

#[test]
fn flag_description_defaults_to_name() {
    assert_eq!(AppFlag::NewCheckout.description(), AppFlag::NewCheckout.name());
    assert_eq!(AppFlag::BetaDashboard.description(), "beta_dashboard");
}
