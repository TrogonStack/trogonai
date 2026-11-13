#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(serde_json_macro, reason = "Tests exercise exact upstream JSON fixtures.")
)]

use super::*;
use serde_json::json;

#[test]
fn a_returned_token_is_sent_alone() {
    assert_eq!(
        OAuthTokenResponse::token("t").to_wire_json(),
        json!({"accessToken": "t"})
    );
}

#[test]
fn a_decline_without_a_token_carries_the_reason() {
    assert_eq!(
        OAuthTokenResponse::declined(OAuthDeclineReason::IdentityChanged).to_wire_json(),
        json!({"accessToken": null, "reason": "identity_changed"})
    );
}

#[test]
fn a_decline_next_to_a_token_is_dropped() {
    let response = OAuthTokenResponse {
        access_token: Some("t".into()),
        decline_reason: Some(OAuthDeclineReason::Transient),
    };
    assert_eq!(response.to_wire_json(), json!({"accessToken": "t"}));
}

#[test]
fn nothing_returned_sends_a_null_token() {
    assert_eq!(
        OAuthTokenResponse::default().to_wire_json(),
        json!({"accessToken": null})
    );
}

#[test]
fn only_known_token_states_are_forwarded() {
    assert_eq!(
        OAuthTokenState::from_wire(Some("refused")),
        Some(OAuthTokenState::Refused)
    );
    assert_eq!(
        OAuthTokenState::from_wire(Some("not_refused")),
        Some(OAuthTokenState::NotRefused)
    );
    assert_eq!(OAuthTokenState::from_wire(Some("other")), None);
    assert_eq!(OAuthTokenState::from_wire(None), None);
}

#[test]
fn the_host_token_is_wrapped_or_null() {
    assert_eq!(host_auth_wire_json(&Some("h".into())), json!({"authToken": "h"}));
    assert_eq!(host_auth_wire_json(&None), json!({"authToken": null}));
}

#[test]
fn every_decline_reason_has_its_upstream_spelling() {
    let spellings = [
        OAuthDeclineReason::SignedOut,
        OAuthDeclineReason::IdentityChanged,
        OAuthDeclineReason::Transient,
        OAuthDeclineReason::RefreshFailed,
    ]
    .map(OAuthDeclineReason::as_str);
    assert_eq!(
        spellings,
        ["signed_out", "identity_changed", "transient", "refresh_failed"]
    );
}

#[test]
fn handler_trait_objects_debug_without_exposing_the_closure() {
    let oauth: std::sync::Arc<dyn OAuthTokenHandler> =
        std::sync::Arc::new(|_: OAuthTokenRequest| async { Ok(OAuthTokenResponse::default()) });
    let host: std::sync::Arc<dyn HostAuthTokenHandler> =
        std::sync::Arc::new(|_: HostAuthTokenRequest| async { Ok(HostAuthToken::default()) });
    assert!(format!("{:?}", &*oauth).starts_with("OAuthTokenHandler"));
    assert!(format!("{:?}", &*host).starts_with("HostAuthTokenHandler"));
}
