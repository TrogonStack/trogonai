use super::*;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::Implementation;
use futures::FutureExt;

fn from(name: &str) -> ClientInitialize {
    InitializeRequest::new(ProtocolVersion::V1)
        .client_info(Implementation::new(name, "1.0.0"))
        .into()
}

fn answered(request: ClientInitialize) -> Handshake {
    Handshake {
        response: InitializeResponse::new(ProtocolVersion::V1),
        request,
    }
}

fn wants_relaunch(handshake: &ClientHandshake) -> bool {
    handshake.relaunch_wanted().now_or_never().is_some()
}

#[test]
fn an_initialized_child_keeps_its_initialize_for_the_next_child() {
    let handshake = ClientHandshake::default();
    assert_eq!(handshake.next(), None);

    handshake.initialized(&answered(from("discord")));

    assert_eq!(handshake.next(), Some(from("discord")));
    assert!(!wants_relaunch(&handshake), "the child already has it");
}

#[test]
fn a_refused_initialize_is_dropped_so_the_next_child_waits_for_the_client() {
    let handshake = ClientHandshake::default();
    let _pending = handshake.relaunch_with(from("discord"));

    handshake.settle(&from("discord"), &Err(Error::invalid_params()));

    assert_eq!(handshake.next(), None);
}

#[test]
fn a_child_that_did_not_answer_in_time_refused_nothing_so_the_initialize_is_kept() {
    let handshake = ClientHandshake::default();
    let _pending = handshake.relaunch_with(from("discord"));

    handshake.time_out(&from("discord"));

    assert_eq!(handshake.next(), Some(from("discord")));
}

#[test]
fn a_refusal_of_an_older_initialize_keeps_the_newer_one() {
    let handshake = ClientHandshake::default();
    let _pending = handshake.relaunch_with(from("discord-2"));

    handshake.settle(&from("discord-1"), &Err(Error::invalid_params()));

    assert_eq!(handshake.next(), Some(from("discord-2")));
}

#[tokio::test]
async fn a_pending_initialize_gets_the_answer_to_its_own_request() {
    let handshake = ClientHandshake::default();
    let pending = handshake.relaunch_with(from("discord"));
    let reply = InitializeResponse::new(ProtocolVersion::V1);

    handshake.settle(&from("other"), &Ok(InitializeResponse::new(ProtocolVersion::V0)));
    handshake.settle(&from("discord"), &Ok(reply.clone()));

    assert_eq!(pending.answer().await.expect("answered"), reply);
}

#[tokio::test]
async fn a_newer_initialize_supersedes_a_pending_one_at_once() {
    let handshake = ClientHandshake::default();
    let older = handshake.relaunch_with(from("discord-1"));
    let newer = handshake.relaunch_with(from("discord-2"));

    let error = older.answer().await.expect_err("superseded");
    assert_eq!(error.code, ErrorCode::INVALID_REQUEST);
    assert!(error.message.contains("newer one"), "{}", error.message);
    assert_eq!(
        handshake.next(),
        Some(from("discord-2")),
        "the next child gets the newer one"
    );

    let reply = InitializeResponse::new(ProtocolVersion::V1);
    handshake.settle(&from("discord-2"), &Ok(reply.clone()));
    assert_eq!(newer.answer().await.expect("answered"), reply);
}

#[tokio::test]
async fn the_same_initialize_twice_waits_on_one_relaunch() {
    let handshake = ClientHandshake::default();
    let first = handshake.relaunch_with(from("discord"));
    let second = handshake.relaunch_with(from("discord"));
    let reply = InitializeResponse::new(ProtocolVersion::V1);

    handshake.settle(&from("discord"), &Ok(reply.clone()));

    assert_eq!(first.answer().await.expect("first"), reply);
    assert_eq!(second.answer().await.expect("second"), reply);
}

#[tokio::test]
async fn an_initialize_a_newer_child_already_answered_is_answered_at_once() {
    let handshake = ClientHandshake::default();
    let _pending = handshake.relaunch_with(from("discord"));
    let reply = InitializeResponse::new(ProtocolVersion::V1);
    handshake.settle(&from("discord"), &Ok(reply.clone()));

    let late = handshake.relaunch_with(from("discord"));

    assert!(!wants_relaunch(&handshake), "no second relaunch for it");
    assert_eq!(late.answer().await.expect("answered"), reply);
}

#[test]
fn a_relaunch_is_wanted_until_a_child_answers_the_pending_initialize() {
    let handshake = ClientHandshake::default();
    handshake.initialized(&answered(from("discord-1")));

    let _pending = handshake.relaunch_with(from("discord-2"));
    assert!(wants_relaunch(&handshake));

    handshake.settle(&from("discord-2"), &Ok(InitializeResponse::new(ProtocolVersion::V1)));
    assert!(!wants_relaunch(&handshake));
}

#[tokio::test(start_paused = true)]
async fn an_initialize_no_child_answers_times_out() {
    let handshake = ClientHandshake::default();
    let pending = handshake.relaunch_with(from("discord"));

    let error = pending.answer().await.expect_err("times out");

    assert!(error.message.contains("did not answer"), "{}", error.message);
}

#[test]
fn members_the_schema_does_not_name_are_kept_and_compared() {
    let offering = |tool: &str| {
        ClientInitialize::from_params(serde_json::json!({
            "protocolVersion": 1,
            "clientCapabilities": {"libtrogonai": {"tools": [{"name": tool}]}},
            "clientInfo": {"name": "discord", "version": "1"},
        }))
        .expect("an initialize")
    };

    let request = offering("send_message");

    assert_eq!(
        request.params()["clientCapabilities"]["libtrogonai"]["tools"][0]["name"],
        "send_message"
    );
    assert_eq!(
        request.typed().client_info.as_ref().map(|info| info.name.as_str()),
        Some("discord")
    );
    assert_eq!(request, offering("send_message"));
    assert_ne!(request, offering("react"));
}

#[test]
fn params_that_are_no_initialize_are_refused_as_bad_params() {
    let error = ClientInitialize::from_params(serde_json::json!({"clientInfo": 7})).expect_err("not an initialize");

    assert_eq!(error.code, agent_client_protocol::ErrorCode::INVALID_PARAMS);
}
