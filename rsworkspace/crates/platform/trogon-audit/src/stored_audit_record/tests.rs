use super::*;

#[test]
fn holds_the_payload_and_stream_sequence_without_metadata() {
    let record = StoredAuditRecord::new(7, vec![1, 2, 3]);

    assert_eq!(record.stream_sequence(), 7);
    assert_eq!(record.payload(), &Bytes::from_static(&[1, 2, 3]));
    assert_eq!(record.subject(), None);
    assert_eq!(record.message_id(), None);
    assert_eq!(record.writer(), None);
    assert_eq!(record.sequence(), None);
    assert_eq!(record.chain(), None);
    assert!(!record.has_metadata());
}

#[test]
fn takes_the_provenance_headers() {
    let mut headers = HeaderMap::new();
    headers.insert(NATS_MESSAGE_ID, "record-1");
    headers.insert(HEADER_WRITER, "gateway-1");
    headers.insert(HEADER_SEQUENCE, "3");
    headers.insert(HEADER_CHAIN, "chain-1");
    headers.insert("Other", "ignored");

    let record = StoredAuditRecord::new(7, Bytes::new())
        .with_subject("platform.v1.audit.decision.project_1.deny")
        .with_headers(&headers);

    assert_eq!(record.subject(), Some("platform.v1.audit.decision.project_1.deny"));
    assert_eq!(record.message_id(), Some("record-1"));
    assert_eq!(record.writer(), Some("gateway-1"));
    assert_eq!(record.sequence(), Some("3"));
    assert_eq!(record.chain(), Some("chain-1"));
    assert!(record.has_metadata());
}

const HEADERS: [(&str, &str); 4] = [
    ("Nats-Msg-Id", "record-1"),
    (HEADER_WRITER, "gateway-1"),
    (HEADER_SEQUENCE, "3"),
    (HEADER_CHAIN, "chain-1"),
];

fn headers_without(missing: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in HEADERS {
        if Some(name) != missing {
            headers.insert(name, value);
        }
    }
    headers
}

#[test]
fn the_message_id_header_is_the_nats_one() {
    assert_eq!(HEADERS[0].0, AsRef::<str>::as_ref(&NATS_MESSAGE_ID));
}

#[test]
fn any_missing_piece_of_metadata_leaves_it_unchecked() {
    let complete = StoredAuditRecord::new(7, Bytes::new())
        .with_subject("s")
        .with_headers(&headers_without(None));
    assert!(complete.has_metadata());

    for (header, _) in HEADERS {
        let record = StoredAuditRecord::new(7, Bytes::new())
            .with_subject("s")
            .with_headers(&headers_without(Some(header)));
        assert!(!record.has_metadata(), "{header}");
    }
    let no_subject = StoredAuditRecord::new(7, Bytes::new()).with_headers(&headers_without(None));
    assert!(!no_subject.has_metadata());
}
