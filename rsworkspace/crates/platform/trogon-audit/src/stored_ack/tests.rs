use super::*;

#[test]
fn keeps_what_the_stream_answered() {
    let ack = StoredAck::new("AUDIT", 12, true);
    assert_eq!(ack.stream(), "AUDIT");
    assert_eq!(ack.stream_sequence(), 12);
    assert!(ack.duplicate());
}

#[test]
fn is_read_from_a_jetstream_publish_ack() {
    let ack = StoredAck::from(PublishAck {
        stream: "AUDIT".to_owned(),
        sequence: 7,
        domain: String::new(),
        duplicate: false,
        value: None,
    });
    assert_eq!(ack, StoredAck::new("AUDIT", 7, false));
}
