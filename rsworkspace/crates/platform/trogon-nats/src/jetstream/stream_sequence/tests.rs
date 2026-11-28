use super::*;

fn with_reply(reply: Option<&str>) -> async_nats::Message {
    async_nats::Message {
        subject: "acp.v1.session.s1.agent.cancel".into(),
        reply: reply.map(Into::into),
        payload: bytes::Bytes::new(),
        headers: None,
        status: None,
        description: None,
        length: 0,
    }
}

#[test]
fn reads_the_original_reply_layout() {
    let message = with_reply(Some("$JS.ACK.ACP_COMMANDS.AGENT_COMMANDS.1.42.7.1700000000000000000.0"));
    assert_eq!(stream_sequence(&message), Some(42));
}

#[test]
fn reads_the_layout_with_domain_and_account_hash() {
    let message = with_reply(Some(
        "$JS.ACK._.HASH.ACP_COMMANDS.AGENT_COMMANDS.1.42.7.1700000000000000000.0",
    ));
    assert_eq!(stream_sequence(&message), Some(42));

    let with_token = with_reply(Some(
        "$JS.ACK.hub.HASH.ACP_COMMANDS.AGENT_COMMANDS.1.42.7.1700000000000000000.0.tok",
    ));
    assert_eq!(stream_sequence(&with_token), Some(42));
}

#[test]
fn rejects_replies_that_are_not_jetstream_acks() {
    assert_eq!(stream_sequence(&with_reply(None)), None);
    assert_eq!(stream_sequence(&with_reply(Some("_INBOX.abc"))), None);
    assert_eq!(stream_sequence(&with_reply(Some("$JS.ACK.S.C.1.2"))), None);
    assert_eq!(stream_sequence(&with_reply(Some("$JS.ACK.S.C.1.x.7.0.0"))), None);
}
