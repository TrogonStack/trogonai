use super::*;

fn fixture() -> Fixture {
    fixture_expecting("publish")
}

fn fixture_expecting(outcome: &str) -> Fixture {
    serde_json::from_value(serde_json::json!({
        "id": "http.synthetic",
        "kind": "event",
        "transport": "http",
        "provenance": {
            "bolt_version": "0.0.0",
            "bolt_commit": "0000000000000000000000000000000000000000",
            "payload_source": "synthetic"
        },
        "not_representable": null,
        "input": { "headers": {}, "body": "{}" },
        "expected": { "outcome": outcome, "nats": null, "http": null, "socket_ack": null }
    }))
    .expect("synthetic fixture")
}

fn header_diff(name: &str) -> Diff {
    Diff {
        field: Field::MissingHeader(name.to_owned()),
        expected: "x".to_owned(),
        actual: "missing".to_owned(),
    }
}

fn header_value_diff(name: &str) -> Diff {
    Diff {
        field: Field::HeaderValue(name.to_owned()),
        expected: "x".to_owned(),
        actual: "y".to_owned(),
    }
}

fn unroutable_diff() -> Diff {
    Diff {
        field: Field::Unroutable,
        expected: "0".to_owned(),
        actual: "1".to_owned(),
    }
}

fn gaps(ids: &[GapId]) -> LedgerEntry {
    LedgerEntry::Gaps(ids.iter().copied().collect())
}

#[test]
fn a_mismatch_without_a_ledger_entry_fails() {
    let problems = judge(&fixture(), None, &[header_diff(HEADER_ENVELOPE_ID)]);
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("not covered"));
}

#[test]
fn a_ledger_entry_that_no_longer_matches_anything_is_stale() {
    let problems = judge(&fixture(), Some(&gaps(&[GapId::G13])), &[]);
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("stale"));
}

#[test]
fn a_gap_that_explains_a_different_mismatch_is_stale_and_leaves_the_mismatch_uncovered() {
    let problems = judge(
        &fixture(),
        Some(&gaps(&[GapId::G22])),
        &[header_diff(HEADER_ENVELOPE_ID)],
    );
    assert_eq!(problems.len(), 2);
}

#[test]
fn a_matching_gap_covers_its_mismatch() {
    let problems = judge(
        &fixture(),
        Some(&gaps(&[GapId::G13])),
        &[header_diff(HEADER_ENVELOPE_ID)],
    );
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn a_representable_fixture_cannot_be_marked_not_representable() {
    let entry = LedgerEntry::NotRepresentable(NotRepresentableMarker::NotRepresentable);
    assert_eq!(judge(&fixture(), Some(&entry), &[]).len(), 1);
}

#[test]
fn a_missing_header_gap_does_not_excuse_a_wrong_header_value() {
    let diff = header_value_diff(HEADER_ENVELOPE_ID);
    let problems = judge(&fixture(), Some(&gaps(&[GapId::G13])), &[diff]);
    assert_eq!(problems.len(), 2, "{problems:?}");
}

#[test]
fn an_event_metadata_header_is_not_a_delivery_identifier_header() {
    let diff = header_diff(HEADERS_EVENT_METADATA[0]);
    assert_eq!(
        judge(&fixture(), Some(&gaps(&[GapId::G13])), std::slice::from_ref(&diff)).len(),
        2
    );
    assert!(judge(&fixture(), Some(&gaps(&[GapId::G22])), &[diff]).is_empty());
}

fn expected_nats(headers: serde_json::Value) -> ExpectedNats {
    serde_json::from_value(serde_json::json!({
        "subject_suffix": "event.message",
        "dedup_id": null,
        "payload": "{}",
        "payload_fill": null,
        "headers": headers
    }))
    .expect("synthetic nats expectation")
}

fn delivery(headers: &[(&str, &str)]) -> Delivery {
    Delivery {
        subject: "slack.event.message".to_owned(),
        headers: headers
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
        payload: Bytes::from_static(b"{}"),
    }
}

fn http_fixture(challenge: &str) -> Fixture {
    serde_json::from_value(serde_json::json!({
        "id": "http.synthetic",
        "kind": "url_verification",
        "transport": "http",
        "provenance": {
            "bolt_version": "0.0.0",
            "bolt_commit": "0000000000000000000000000000000000000000",
            "payload_source": "synthetic"
        },
        "not_representable": null,
        "input": { "headers": {}, "body": "{}" },
        "expected": {
            "outcome": "inline",
            "nats": null,
            "http": { "status": 200, "body": null, "challenge": challenge },
            "socket_ack": null
        }
    }))
    .expect("synthetic fixture")
}

fn observed_body(body: &str) -> Observed {
    Observed {
        outcome: Outcome::Inline,
        deliveries: Vec::new(),
        unroutable: 0,
        status: Some(200),
        body: Some(Bytes::copy_from_slice(body.as_bytes())),
        socket_ack: None,
        failure: None,
    }
}

#[test]
fn a_header_with_the_wrong_value_and_an_unexpected_header_are_distinct_mismatches() {
    let expected = expected_nats(serde_json::json!({ "X-Slack-Team-Id": "T1" }));
    let delivery = delivery(&[("X-Slack-Team-Id", "T2"), ("X-Slack-Extra", "1")]);
    let mut diffs = Vec::new();
    compare_delivery(&expected, &delivery, &mut diffs);
    let fields: Vec<&Field> = diffs.iter().map(|diff| &diff.field).collect();
    assert_eq!(
        fields,
        [
            &Field::HeaderValue("X-Slack-Team-Id".to_owned()),
            &Field::UnexpectedHeader("X-Slack-Extra".to_owned()),
        ]
    );
}

#[test]
fn a_challenge_must_come_back_whole_in_a_supported_shape() {
    let fixture = http_fixture("abc");
    for body in ["abc", "{\"challenge\":\"abc\"}", "challenge=abc"] {
        assert!(compare(&fixture, &observed_body(body)).is_empty(), "{body}");
    }
    for body in [
        "abc\n",
        "abcd",
        "{\"challenge\":\"abc\",\"x\":1}",
        "{\"challenge\":\"abcd\"}",
        "challenge=abc&x=1",
        "",
    ] {
        let diffs = compare(&fixture, &observed_body(body));
        assert_eq!(diffs.len(), 1, "{body}");
        assert_eq!(diffs[0].field, Field::ResponseBody, "{body}");
    }
}

#[test]
fn an_unroutable_record_is_only_explained_when_bolt_rejects_the_request() {
    let gap = gaps(&[GapId::G23]);
    assert!(judge(&fixture_expecting("reject"), Some(&gap), &[unroutable_diff()]).is_empty());
    assert_eq!(
        judge(&fixture_expecting("inline"), Some(&gap), &[unroutable_diff()]).len(),
        2
    );
    assert_eq!(judge(&fixture(), None, &[unroutable_diff()]).len(), 1);
}
