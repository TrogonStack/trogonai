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

fn unroutable_diff() -> Diff {
    Diff {
        field: Field::Unroutable,
        expected: "0".to_owned(),
        actual: "1".to_owned(),
    }
}

fn diff_on(field: Field) -> Diff {
    Diff {
        field,
        expected: "x".to_owned(),
        actual: "y".to_owned(),
    }
}

fn gaps(ids: &[GapId]) -> LedgerEntry {
    LedgerEntry::Gaps(ids.iter().copied().collect())
}

#[test]
fn a_mismatch_without_a_ledger_entry_fails() {
    let problems = judge(
        &fixture(),
        None,
        Some(&observed_body("")),
        &[header_diff("X-Slack-Extra")],
    );
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("not covered"));
}

#[test]
fn a_ledger_entry_that_no_longer_matches_anything_is_stale() {
    let problems = judge(&fixture(), Some(&gaps(&[GapId::G23])), Some(&observed_body("")), &[]);
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("stale"));
}

#[test]
fn a_gap_that_explains_a_different_mismatch_is_stale_and_leaves_the_mismatch_uncovered() {
    let problems = judge(
        &fixture(),
        Some(&gaps(&[GapId::G23])),
        Some(&observed_body("")),
        &[header_diff("X-Slack-Extra")],
    );
    assert_eq!(problems.len(), 2);
}

#[test]
fn a_matching_gap_covers_its_mismatch() {
    let problems = judge(
        &fixture_expecting("reject"),
        Some(&gaps(&[GapId::G23])),
        Some(&observed_body("")),
        &[unroutable_diff()],
    );
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn a_representable_fixture_cannot_be_marked_not_representable() {
    let entry = LedgerEntry::NotRepresentable(NotRepresentableMarker::NotRepresentable);
    assert_eq!(judge(&fixture(), Some(&entry), Some(&observed_body("")), &[]).len(), 1);
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

fn rejected(status: u16) -> Observed {
    let mut observed = observed_body("");
    observed.outcome = Outcome::Reject;
    observed.status = Some(status);
    observed
}

fn observed_body(body: &str) -> Observed {
    Observed {
        outcome: Outcome::Inline,
        requests: Vec::new(),
        content_type: None,
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
    assert!(
        judge(
            &fixture_expecting("reject"),
            Some(&gap),
            Some(&observed_body("")),
            &[unroutable_diff()]
        )
        .is_empty()
    );
    assert_eq!(
        judge(
            &fixture_expecting("inline"),
            Some(&gap),
            Some(&observed_body("")),
            &[unroutable_diff()]
        )
        .len(),
        2
    );
    assert_eq!(
        judge(&fixture(), None, Some(&observed_body("")), &[unroutable_diff()]).len(),
        1
    );
}

fn request_fixture(content_type: serde_json::Value) -> Fixture {
    serde_json::from_value(serde_json::json!({
        "id": "http.synthetic",
        "kind": "command",
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
            "request": {
                "subject_suffix": "respond.command.deploy",
                "headers": { "X-Slack-Team-Id": "T1" },
                "payload": "{}",
                "reply": { "content_type": null, "body": "" }
            },
            "http": { "status": 200, "body": null, "content_type": content_type, "challenge": null },
            "socket_ack": null
        }
    }))
    .expect("synthetic fixture")
}

fn requested(headers: &[(&str, &str)], payload: &'static [u8]) -> Requested {
    Requested {
        headers: headers
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
        payload: Bytes::from_static(payload),
    }
}

fn fields(diffs: &[Diff]) -> Vec<&Field> {
    diffs.iter().map(|diff| &diff.field).collect()
}

#[test]
fn a_matching_request_leaves_no_mismatch() {
    let fixture = request_fixture(serde_json::Value::Null);
    let mut observed = observed_body("");
    observed.requests = vec![requested(&[("X-Slack-Team-Id", "T1"), (DEDUP_HEADER, "d")], b"{}")];
    assert!(compare(&fixture, &observed).is_empty());
}

#[test]
fn a_request_is_compared_on_count_payload_and_headers() {
    let fixture = request_fixture(serde_json::Value::Null);

    let missing = compare(&fixture, &observed_body(""));
    assert_eq!(fields(&missing), [&Field::Requests]);

    let mut observed = observed_body("");
    observed.requests = vec![requested(&[("X-Slack-Team-Id", "T2"), ("X-Slack-Extra", "1")], b"[]")];
    let diffs = compare(&fixture, &observed);
    assert_eq!(
        fields(&diffs),
        [
            &Field::RequestPayload,
            &Field::RequestHeader("X-Slack-Team-Id".to_owned()),
            &Field::RequestHeader("X-Slack-Extra".to_owned()),
        ]
    );

    let mut observed = observed_body("");
    observed.requests = vec![requested(&[], b"{}")];
    let diffs = compare(&fixture, &observed);
    assert_eq!(fields(&diffs), [&Field::RequestHeader("X-Slack-Team-Id".to_owned())]);
}

#[test]
fn a_request_nobody_expected_is_a_mismatch() {
    let mut observed = observed_body("");
    observed.requests = vec![requested(&[], b"{}")];
    let diffs = compare(&fixture_expecting("inline"), &observed);
    assert_eq!(fields(&diffs), [&Field::Requests]);
}

#[test]
fn an_expected_content_type_must_be_the_one_observed() {
    let fixture = request_fixture(serde_json::json!("application/json"));
    let mut observed = observed_body("");
    observed.requests = vec![requested(&[("X-Slack-Team-Id", "T1")], b"{}")];
    assert_eq!(fields(&compare(&fixture, &observed)), [&Field::ContentType]);

    observed.content_type = Some("text/plain".to_owned());
    assert_eq!(fields(&compare(&fixture, &observed)), [&Field::ContentType]);

    observed.content_type = Some("application/json".to_owned());
    assert!(compare(&fixture, &observed).is_empty());
}

#[test]
fn a_future_timestamp_gap_only_explains_an_unauthorized_rejection_of_a_request_bolt_accepts() {
    let gap = gaps(&[GapId::G24]);
    let diffs = [diff_on(Field::Outcome), diff_on(Field::Status)];
    let unauthorized = rejected(GATEWAY_UNAUTHORIZED_STATUS);
    assert!(judge(&fixture(), Some(&gap), Some(&unauthorized), &diffs).is_empty());
    assert_eq!(
        judge(&fixture_expecting("reject"), Some(&gap), Some(&unauthorized), &diffs).len(),
        3
    );
    assert_eq!(
        judge(&fixture(), Some(&gap), Some(&unauthorized), &[diff_on(Field::Subject)]).len(),
        2
    );
    assert_eq!(judge(&fixture(), Some(&gap), Some(&rejected(500)), &diffs).len(), 3);
    let mut delivered = rejected(GATEWAY_UNAUTHORIZED_STATUS);
    delivered.deliveries.push(delivery(&[]));
    assert_eq!(judge(&fixture(), Some(&gap), Some(&delivered), &diffs).len(), 3);
}

#[test]
fn a_missing_event_id_gap_only_explains_a_dead_letter() {
    let gap = gaps(&[GapId::G25]);
    let diffs = [
        diff_on(Field::Outcome),
        diff_on(Field::SocketAck),
        diff_on(Field::Unroutable),
    ];
    let mut dead_letter = rejected(400);
    dead_letter.unroutable = 1;
    assert!(judge(&fixture(), Some(&gap), Some(&dead_letter), &diffs).is_empty());
    assert_eq!(
        judge(&fixture_expecting("inline"), Some(&gap), Some(&dead_letter), &diffs).len(),
        4
    );
    let mut silent_drop = rejected(400);
    silent_drop.outcome = Outcome::Ignore;
    let drop_diffs = [diff_on(Field::Outcome), diff_on(Field::SocketAck)];
    assert_eq!(judge(&fixture(), Some(&gap), Some(&silent_drop), &drop_diffs).len(), 3);
}

#[test]
fn a_lenient_content_type_gap_only_explains_an_http_request_the_gateway_processed() {
    let gap = gaps(&[GapId::G26]);
    let diffs = [
        diff_on(Field::Outcome),
        diff_on(Field::Status),
        diff_on(Field::Requests),
        diff_on(Field::Deliveries),
    ];
    let mut accepted = observed_body("");
    accepted.outcome = Outcome::Publish;
    accepted.deliveries.push(delivery(&[]));
    assert!(judge(&fixture_expecting("reject"), Some(&gap), Some(&accepted), &diffs).is_empty());
    assert_eq!(judge(&fixture(), Some(&gap), Some(&accepted), &diffs).len(), 5);
    let mut unanswered = rejected(404);
    unanswered.requests.push(Requested {
        headers: BTreeMap::new(),
        payload: Bytes::new(),
    });
    assert!(judge(&fixture_expecting("reject"), Some(&gap), Some(&unanswered), &diffs).is_empty());
    let status_diffs = [diff_on(Field::Status)];
    assert_eq!(
        judge(
            &fixture_expecting("reject"),
            Some(&gap),
            Some(&rejected(500)),
            &status_diffs
        )
        .len(),
        2
    );
}

#[test]
fn an_unanswered_body_gap_only_explains_an_immediate_dead_letter() {
    let gap = gaps(&[GapId::G27]);
    let diffs = [diff_on(Field::Outcome), diff_on(Field::Status)];
    let mut unhandled = fixture_expecting("reject");
    unhandled.expected.http = Some(ExpectedHttp {
        status: BOLT_UNHANDLED_STATUS,
        body: None,
        content_type: None,
        challenge: None,
    });
    let mut dead_letter = observed_body("");
    dead_letter.unroutable = 1;
    assert!(judge(&unhandled, Some(&gap), Some(&dead_letter), &diffs).is_empty());
    assert_eq!(
        judge(&fixture_expecting("reject"), Some(&gap), Some(&dead_letter), &diffs).len(),
        3
    );
    let mut failed = rejected(500);
    failed.unroutable = 1;
    assert_eq!(judge(&unhandled, Some(&gap), Some(&failed), &diffs).len(), 3);
}

#[test]
fn a_representable_fixture_without_an_observation_fails() {
    let problems = judge(&fixture(), None, None, &[]);
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("never observed"));
}
