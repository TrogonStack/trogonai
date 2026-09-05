use super::*;
use agent_client_protocol::Error as AcpError;

mod lifecycle_tests;

/// An initialize response carrying the given agent capabilities, deserialized
/// rather than constructed because the schema types are `#[non_exhaustive]`. This
/// also puts the wire names under test, which is where a capability gets misread.
fn initialized(agent_capabilities: serde_json::Value) -> InitializeResponse {
    serde_json::from_value(serde_json::json!({
        "protocolVersion": 1,
        "agentCapabilities": agent_capabilities,
    }))
    .expect("initialize response")
}

fn all_advertised() -> InitializeResponse {
    initialized(serde_json::json!({
        "loadSession": true,
        "sessionCapabilities": {
            "list": {},
            "delete": {},
            "resume": {},
            "close": {},
            "additionalDirectories": {},
        },
    }))
}

#[test]
fn every_advertised_method_is_read_as_supported() {
    let methods = SessionMethods::advertised(&all_advertised());

    for method in SessionMethod::ALL {
        assert!(methods.supports(method), "{} should be supported", method.wire_name());
    }
}

/// The inversion this guards: ACP capabilities are present-means-supported, so an
/// agent that advertises nothing must come back as supporting nothing rather than
/// as supporting everything.
#[test]
fn an_agent_that_advertises_nothing_supports_nothing() {
    let methods = SessionMethods::advertised(&initialized(serde_json::json!({})));

    for method in SessionMethod::ALL {
        assert!(
            !methods.supports(method),
            "{} should not be supported",
            method.wire_name()
        );
    }
    assert_eq!(SessionMethods::default().to_string(), "none");
}

/// `null` is how an agent declines a capability it knows about, and it has to read
/// the same as leaving the key out entirely.
#[test]
fn a_null_capability_is_declined_not_advertised() {
    let methods = SessionMethods::advertised(&initialized(serde_json::json!({
        "loadSession": false,
        "sessionCapabilities": {
            "list": null,
            "delete": null,
            "resume": null,
            "close": {},
            "additionalDirectories": null,
        },
    })));

    assert!(methods.supports(SessionMethod::Close));
    for method in [
        SessionMethod::Load,
        SessionMethod::List,
        SessionMethod::Delete,
        SessionMethod::Resume,
        SessionMethod::AdditionalDirectories,
    ] {
        assert!(
            !methods.supports(method),
            "{} should not be supported",
            method.wire_name()
        );
    }
}

#[test]
fn display_lists_the_advertised_methods_by_wire_name() {
    assert_eq!(
        SessionMethods::advertised(&all_advertised()).to_string(),
        "session/load, session/list, session/delete, session/resume, session/close, additionalDirectories"
    );
    assert_eq!(
        SessionMethods::advertised(&initialized(serde_json::json!({
            "sessionCapabilities": { "close": {} },
        })))
        .to_string(),
        "session/close"
    );
}

/// The classification the caller treats as a hint: broad enough to cover the codes
/// an agent rejects an unknown session id with, and no broader, so a transport
/// failure or an unimplemented method does not trigger a session rotation.
#[test]
fn only_a_rejected_session_id_reads_as_a_lost_session() {
    for error in [AcpError::invalid_params(), AcpError::resource_not_found(None)] {
        let message = error.message.clone();
        assert!(
            AcpPortError::Rpc(error).is_session_lost(),
            "{message} should read as a lost session"
        );
    }

    for error in [
        AcpError::internal_error(),
        AcpError::method_not_found(),
        AcpError::request_cancelled(),
        AcpError::auth_required(),
        AcpError::invalid_request(),
        AcpError::parse_error(),
    ] {
        let message = error.message.clone();
        assert!(
            !AcpPortError::Rpc(error).is_session_lost(),
            "{message} should not read as a lost session"
        );
    }
}

/// An id the agent handed back that is not a usable token names a session the
/// port has already handed straight back, so there is nothing for a fresh one to
/// repair. Reading it as a lost session would have the pipeline open a
/// replacement against an agent that is going to name the next one just as
/// unusably.
#[test]
fn an_unusable_session_id_is_not_a_lost_session() {
    let error = trogon_channel::AgentSessionId::new("sess 1").expect_err("an id with a space is not a token");
    assert!(!AcpPortError::SessionId(error).is_session_lost());
}
