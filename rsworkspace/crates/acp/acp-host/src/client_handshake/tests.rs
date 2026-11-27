use super::*;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::Implementation;

fn from(name: &str) -> InitializeRequest {
    InitializeRequest::new(ProtocolVersion::V1).client_info(Implementation::new(name, "1.0.0"))
}

#[test]
fn a_remembered_initialize_is_kept_for_the_next_child() {
    let handshake = ClientHandshake::default();
    assert_eq!(handshake.last(), None);

    handshake.remember(from("discord"));

    assert_eq!(handshake.last(), Some(from("discord")));
}

#[test]
fn a_refused_initialize_is_dropped_so_the_next_child_waits_for_the_client() {
    let handshake = ClientHandshake::default();
    handshake.remember(from("discord"));

    handshake.settle(&from("discord"), &Err(Error::invalid_params()));

    assert_eq!(handshake.last(), None);
}

#[test]
fn a_refusal_of_an_older_initialize_keeps_the_newer_one() {
    let handshake = ClientHandshake::default();
    handshake.remember(from("discord-2"));

    handshake.settle(&from("discord-1"), &Err(Error::invalid_params()));

    assert_eq!(handshake.last(), Some(from("discord-2")));
}

#[tokio::test]
async fn a_pending_initialize_gets_the_answer_to_its_own_request() {
    let handshake = ClientHandshake::default();
    let pending = handshake.listen();
    let reply = InitializeResponse::new(ProtocolVersion::V1);

    handshake.settle(&from("other"), &Ok(InitializeResponse::new(ProtocolVersion::V0)));
    handshake.settle(&from("discord"), &Ok(reply.clone()));

    assert_eq!(pending.answer(&from("discord")).await.expect("answered"), reply);
}

#[tokio::test(start_paused = true)]
async fn an_answer_recorded_before_listening_is_not_taken_for_a_new_one() {
    let handshake = ClientHandshake::default();
    handshake.settle(&from("discord"), &Err(Error::invalid_params()));
    let pending = handshake.listen();

    let error = pending.answer(&from("discord")).await.expect_err("times out");

    assert!(error.message.contains("did not answer"), "{}", error.message);
}
