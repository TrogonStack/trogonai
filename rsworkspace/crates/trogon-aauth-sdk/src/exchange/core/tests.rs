#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use trogon_identity_types::aauth::error::PollingError;

use super::*;

fn pending_headers(location: &str, retry_after_secs: u64) -> PendingHeaders {
    PendingHeaders {
        location: location.to_string(),
        retry_after_secs,
        requirement: None,
    }
}

#[test]
fn initial_granted_transitions_straight_to_granted() {
    let grant = TokenGrantResponse {
        auth_token: "eyJ...".to_string(),
        expires_in: 3600,
    };
    let (state, action) = step(ExchangeState::AwaitingInitial, ExchangeEvent::Granted(grant.clone()));
    assert_eq!(state, ExchangeState::Granted(grant));
    assert_eq!(action, ExchangeAction::Stop);
}

#[test]
fn initial_pending_with_no_requirement_starts_polling() {
    let event = ExchangeEvent::Pending {
        headers: pending_headers("/pending/abc123", 0),
        body: PendingResponse::pending(),
    };
    let (state, action) = step(ExchangeState::AwaitingInitial, event);
    assert_eq!(
        state,
        ExchangeState::Polling {
            location: "/pending/abc123".to_string(),
            interval_secs: 0,
        }
    );
    assert_eq!(
        action,
        ExchangeAction::PollAfter {
            location: "/pending/abc123".to_string(),
            after_secs: 0,
        }
    );
}

#[test]
fn initial_pending_with_interaction_requirement_surfaces_url_and_code_and_stops() {
    let mut headers = pending_headers("/pending/abc123", 5);
    headers.requirement = Some(Requirement::Interaction {
        url: "https://ps.example/interaction".to_string(),
        code: Some("A1B2-C3D4".to_string()),
    });
    let event = ExchangeEvent::Pending {
        headers,
        body: PendingResponse::pending(),
    };
    let (state, action) = step(ExchangeState::AwaitingInitial, event);
    assert_eq!(
        state,
        ExchangeState::Interacting {
            location: "/pending/abc123".to_string(),
            interval_secs: 5,
            url: "https://ps.example/interaction".to_string(),
            code: Some("A1B2-C3D4".to_string()),
        }
    );
    assert_eq!(action, ExchangeAction::Stop);
}

#[test]
fn pending_with_clarification_question_surfaces_it_and_stops() {
    let event = ExchangeEvent::Pending {
        headers: pending_headers("/pending/abc123", 0),
        body: PendingResponse {
            status: "pending".to_string(),
            clarification: Some("Why do you need calendar write access?".to_string()),
            timeout: Some(120),
            options: None,
            required_claims: None,
        },
    };
    let (state, action) = step(ExchangeState::AwaitingInitial, event);
    assert_eq!(
        state,
        ExchangeState::AwaitingClarification {
            location: "/pending/abc123".to_string(),
            interval_secs: 0,
            question: "Why do you need calendar write access?".to_string(),
            timeout: Some(120),
            options: None,
        }
    );
    assert_eq!(action, ExchangeAction::Stop);
}

#[test]
fn poll_pending_continues_polling_same_location() {
    let state = ExchangeState::Polling {
        location: "/pending/abc123".to_string(),
        interval_secs: 5,
    };
    let event = ExchangeEvent::Pending {
        headers: pending_headers("/pending/abc123", 5),
        body: PendingResponse::pending(),
    };
    let (new_state, action) = step(state, event);
    assert_eq!(
        new_state,
        ExchangeState::Polling {
            location: "/pending/abc123".to_string(),
            interval_secs: 5,
        }
    );
    assert_eq!(
        action,
        ExchangeAction::PollAfter {
            location: "/pending/abc123".to_string(),
            after_secs: 5,
        }
    );
}

#[test]
fn poll_granted_transitions_to_granted() {
    let state = ExchangeState::Polling {
        location: "/pending/abc123".to_string(),
        interval_secs: 5,
    };
    let grant = TokenGrantResponse {
        auth_token: "eyJ...".to_string(),
        expires_in: 3600,
    };
    let (new_state, action) = step(state, ExchangeEvent::Granted(grant.clone()));
    assert_eq!(new_state, ExchangeState::Granted(grant));
    assert_eq!(action, ExchangeAction::Stop);
}

#[test]
fn poll_denied_is_terminal() {
    let state = ExchangeState::Polling {
        location: "/pending/abc123".to_string(),
        interval_secs: 5,
    };
    let (new_state, action) = step(state, ExchangeEvent::Denied(PollingError::Denied));
    assert_eq!(
        new_state,
        ExchangeState::Terminal(TerminalReason::Denied(PollingError::Denied))
    );
    assert_eq!(action, ExchangeAction::Stop);
}

#[test]
fn poll_expired_is_terminal_but_labeled_expired() {
    let state = ExchangeState::Polling {
        location: "/pending/abc123".to_string(),
        interval_secs: 5,
    };
    let (new_state, action) = step(state, ExchangeEvent::Expired);
    assert_eq!(new_state, ExchangeState::Terminal(TerminalReason::Expired));
    assert_eq!(action, ExchangeAction::Stop);
}

#[test]
fn poll_gone_is_terminal_and_must_not_retry() {
    let state = ExchangeState::Polling {
        location: "/pending/abc123".to_string(),
        interval_secs: 5,
    };
    let (new_state, action) = step(state, ExchangeEvent::Gone);
    assert_eq!(new_state, ExchangeState::Terminal(TerminalReason::Gone));
    assert_eq!(action, ExchangeAction::Stop);
}

#[test]
fn poll_slow_down_increases_interval_by_five_seconds() {
    let state = ExchangeState::Polling {
        location: "/pending/abc123".to_string(),
        interval_secs: 5,
    };
    let (new_state, action) = step(state, ExchangeEvent::SlowDown);
    assert_eq!(
        new_state,
        ExchangeState::Polling {
            location: "/pending/abc123".to_string(),
            interval_secs: 10,
        }
    );
    assert_eq!(
        action,
        ExchangeAction::PollAfter {
            location: "/pending/abc123".to_string(),
            after_secs: 10,
        }
    );
}

#[test]
fn slow_down_while_interacting_preserves_url_and_code() {
    let state = ExchangeState::Interacting {
        location: "/pending/abc123".to_string(),
        interval_secs: 5,
        url: "https://ps.example/interaction".to_string(),
        code: Some("A1B2-C3D4".to_string()),
    };
    let (new_state, _) = step(state, ExchangeEvent::SlowDown);
    assert_eq!(
        new_state,
        ExchangeState::Interacting {
            location: "/pending/abc123".to_string(),
            interval_secs: 10,
            url: "https://ps.example/interaction".to_string(),
            code: Some("A1B2-C3D4".to_string()),
        }
    );
}

#[test]
fn poll_server_error_is_terminal_start_over() {
    let state = ExchangeState::Polling {
        location: "/pending/abc123".to_string(),
        interval_secs: 5,
    };
    let (new_state, action) = step(state, ExchangeEvent::ServerError);
    assert_eq!(new_state, ExchangeState::Terminal(TerminalReason::ServerError));
    assert_eq!(action, ExchangeAction::Stop);
}

#[test]
fn poll_service_unavailable_backs_off_per_retry_after() {
    let state = ExchangeState::Polling {
        location: "/pending/abc123".to_string(),
        interval_secs: 5,
    };
    let (new_state, action) = step(state, ExchangeEvent::ServiceUnavailable { retry_after_secs: 30 });
    assert_eq!(
        new_state,
        ExchangeState::Polling {
            location: "/pending/abc123".to_string(),
            interval_secs: 30,
        }
    );
    assert_eq!(
        action,
        ExchangeAction::PollAfter {
            location: "/pending/abc123".to_string(),
            after_secs: 30,
        }
    );
}

#[test]
fn initial_invalid_request_is_terminal() {
    let err = trogon_identity_types::aauth::error::ErrorResponse::new("invalid_request");
    let (state, action) = step(
        ExchangeState::AwaitingInitial,
        ExchangeEvent::InvalidRequest(err.clone()),
    );
    assert_eq!(state, ExchangeState::Terminal(TerminalReason::InvalidRequest(err)));
    assert_eq!(action, ExchangeAction::Stop);
}

#[test]
fn initial_unauthorized_is_terminal() {
    let err = trogon_identity_types::aauth::error::ErrorResponse::new("invalid_agent_token");
    let (state, action) = step(ExchangeState::AwaitingInitial, ExchangeEvent::Unauthorized(err.clone()));
    assert_eq!(state, ExchangeState::Terminal(TerminalReason::Unauthorized(err)));
    assert_eq!(action, ExchangeAction::Stop);
}

#[test]
fn initial_payment_required_is_terminal_and_carries_location() {
    let headers = pending_headers("/pending/pay1", 0);
    let (state, action) = step(ExchangeState::AwaitingInitial, ExchangeEvent::PaymentRequired(headers));
    assert_eq!(
        state,
        ExchangeState::Terminal(TerminalReason::PaymentRequired {
            location: "/pending/pay1".to_string()
        })
    );
    assert_eq!(action, ExchangeAction::Stop);
}

#[test]
fn initial_service_unavailable_with_no_location_yet_stops_for_driver_to_retry_initial() {
    let (state, action) = step(
        ExchangeState::AwaitingInitial,
        ExchangeEvent::ServiceUnavailable { retry_after_secs: 2 },
    );
    assert_eq!(state, ExchangeState::AwaitingInitial);
    assert_eq!(action, ExchangeAction::Stop);
}
