use super::*;

fn frame(event: &str, id: Option<&str>, data: Option<&str>) -> SseFrame {
    SseFrame {
        event: Some(event.to_owned()),
        id: id.map(str::to_owned),
        data: data.map(str::to_owned),
    }
}

fn client(data: &str) -> SseFrame {
    frame("client_event", None, Some(data))
}

fn messages(events: &[SseEvent]) -> Vec<&Value> {
    events
        .iter()
        .filter_map(|event| match event {
            SseEvent::Message { message, .. } => Some(message),
            _ => None,
        })
        .collect()
}

fn dropped(filter: &SseFrameFilter, category: SseDropCategory) -> u64 {
    filter
        .drop_counts()
        .into_iter()
        .find(|(candidate, _)| *candidate == category)
        .map_or(0, |(_, count)| count)
}

#[test]
fn seeds_the_cursor_from_a_positive_safe_integer() {
    assert_eq!(SseFrameFilter::new(Some(7.0)).resume_sequence_num(), 7);
    assert_eq!(SseFrameFilter::new(Some(7.5)).resume_sequence_num(), 0);
    assert_eq!(SseFrameFilter::new(Some(-1.0)).resume_sequence_num(), 0);
    assert_eq!(SseFrameFilter::new(None).resume_sequence_num(), 0);
    assert_eq!(SseFrameFilter::new(Some(7.0)).last_sequence_num(), Some(7));
    assert_eq!(SseFrameFilter::new(None).last_sequence_num(), None);
}

#[test]
fn a_durable_frame_carries_its_envelope_and_advances_the_cursors() {
    let mut filter = SseFrameFilter::new(None);
    let data = r#"{"event_id":"e","sequence_num":4,"created_at":"t","source":"worker","payload":{"type":"assistant","ccr_event":1}}"#;
    let events = filter.handle_frame(&frame("client_event", Some("3"), Some(data)));
    let envelope = SseCcrEvent {
        event_id: "e".to_owned(),
        sequence_num: 3,
        created_at: Some("t".to_owned()),
        source: Some("worker".to_owned()),
    };
    assert!(matches!(events.as_slice(), [SseEvent::Message { ccr_event: Some(found), .. }] if *found == envelope));
    assert_eq!(filter.resume_sequence_num(), 3);
    assert_eq!(filter.last_sequence_num(), None);
    filter.mark_delivered(Some(&envelope));
    filter.mark_delivered(Some(&envelope));
    filter.mark_delivered(None);
    assert_eq!(filter.last_sequence_num(), Some(3));
}

#[test]
fn the_payload_sequence_number_is_used_without_an_id() {
    let mut filter = SseFrameFilter::new(None);
    let events = filter.handle_frame(&client(
        r#"{"event_id":"e","sequence_num":"12abc","payload":{"type":"user","message":{"content":"hi"}},"source":"x"}"#,
    ));
    assert_eq!(messages(&events).len(), 1);
    assert_eq!(filter.resume_sequence_num(), 12);
}

#[test]
fn frames_without_data_or_of_unknown_kinds_do_nothing() {
    let mut filter = SseFrameFilter::new(None);
    assert!(filter.handle_frame(&frame("client_event", Some("5"), None)).is_empty());
    assert_eq!(filter.resume_sequence_num(), 5);
    assert!(filter.handle_frame(&frame("client_event", None, Some(""))).is_empty());
    assert!(filter.handle_frame(&frame("other", None, Some("{}"))).is_empty());
    assert!(filter.handle_frame(&SseFrame::default()).is_empty());
}

#[test]
fn catch_up_and_delivery_updates_are_reported() {
    let mut filter = SseFrameFilter::new(None);
    let events = filter.handle_frame(&frame("catch_up_truncated", None, Some("x")));
    assert!(matches!(
        events.as_slice(),
        [SseEvent::Log(_), SseEvent::CatchUpTruncated]
    ));
    let events = filter.handle_frame(&frame(
        "delivery_update",
        None,
        Some(r#"{"event_id":"e","status":"s"}"#),
    ));
    assert!(matches!(events.as_slice(), [SseEvent::DeliveryUpdate(_)]));
    let events = filter.handle_frame(&frame("delivery_update", None, Some("{")));
    assert!(matches!(events.as_slice(), [SseEvent::Log(_)]));
    assert_eq!(dropped(&filter, SseDropCategory::MalformedDeliveryUpdate), 1);
}

#[test]
fn unparseable_frames_are_dropped() {
    let mut filter = SseFrameFilter::new(None);
    for data in [
        "{",
        "null",
        r#"{"source":{"toString":1},"payload":{"type":"assistant"}}"#,
    ] {
        filter.handle_frame(&client(data));
    }
    assert_eq!(dropped(&filter, SseDropCategory::UnparseableFrame), 3);
    filter.handle_frame(&frame("ephemeral_event", None, Some("{")));
    assert_eq!(dropped(&filter, SseDropCategory::UnparseableFrame), 4);
    filter.handle_frame(&client(r#"{"sequence_num":{"toString":1}}"#));
    assert_eq!(dropped(&filter, SseDropCategory::UnparseableFrame), 5);
}

#[test]
fn payloads_without_a_string_type_are_dropped() {
    let mut filter = SseFrameFilter::new(None);
    for data in [
        "5",
        r#"{"payload":5}"#,
        r#"{"payload":{"type":1}}"#,
        r#"{"payload":{}}"#,
    ] {
        filter.handle_frame(&client(data));
    }
    assert_eq!(dropped(&filter, SseDropCategory::MissingType), 4);
}

#[test]
fn ephemeral_frames_pass_a_short_allowlist() {
    let mut filter = SseFrameFilter::new(None);
    let ephemeral = |data: &str| frame("ephemeral_event", None, Some(data));
    let events = filter.handle_frame(&ephemeral(r#"{"payload":{"type":"stream_event","ccr_event":1,"x":2}}"#));
    assert_eq!(messages(&events).len(), 1);
    let events = filter.handle_frame(&ephemeral(r#"{"payload":{"type":"sources_changed","ccr_event":1}}"#));
    assert_eq!(messages(&events).len(), 1);
    let status = r#"{"payload":{"type":"system","subtype":"task_summary","uuid":"u","detail":"d","extra":1}}"#;
    let events = filter.handle_frame(&ephemeral(status));
    let kept = messages(&events);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].as_object().map(JsonObject::len), Some(4));
    for data in [
        r#"{"payload":{"type":"system","subtype":"init"}}"#,
        r#"{"payload":{"type":"system"}}"#,
        r#"{"payload":{"type":"assistant","subtype":"thinking_tokens"}}"#,
        r#"{"payload":{"type":"user"}}"#,
    ] {
        assert!(messages(&filter.handle_frame(&ephemeral(data))).is_empty());
    }
    assert_eq!(dropped(&filter, SseDropCategory::EphemeralType), 4);
}

#[test]
fn only_the_worker_may_send_worker_shaped_user_frames() {
    let mut filter = SseFrameFilter::new(None);
    let shaped = |source: &str| {
        client(&format!(
            r#"{{"source":"{source}","payload":{{"type":"user","tool_use_result":1,"message":{{"content":"x"}}}}}}"#
        ))
    };
    assert!(messages(&filter.handle_frame(&shaped("client"))).is_empty());
    assert_eq!(messages(&filter.handle_frame(&shaped("worker"))).len(), 1);
    let malformed = client(r#"{"source":"worker","payload":{"type":"user","message":{"content":[5]}}}"#);
    assert!(messages(&filter.handle_frame(&malformed)).is_empty());
    assert_eq!(dropped(&filter, SseDropCategory::UserWorkerOnly), 1);
    assert_eq!(dropped(&filter, SseDropCategory::MalformedUserContent), 1);
}

#[test]
fn control_responses_are_checked_against_the_requests_this_client_made() {
    let mut filter = SseFrameFilter::new(None);
    let response = |source: &str, id: &str| {
        client(&format!(
            r#"{{"source":"{source}","payload":{{"type":"control_response","response":{{"request_id":"{id}","error_code":1,"keep":2,"pending_permission_requests":[]}}}}}}"#
        ))
    };
    assert!(
        filter
            .prepare_write(r#"{"type":"control_request","request_id":"r1"}"#, || "u".to_owned())
            .is_ok()
    );
    assert_eq!(filter.expect_control_response("r2"), None);
    assert!(messages(&filter.handle_frame(&response("client", "r1"))).is_empty());
    assert!(messages(&filter.handle_frame(&response("client", "r2"))).is_empty());
    let events = filter.handle_frame(&response("client", "other"));
    let kept = messages(&events);
    assert_eq!(kept[0].pointer("/response/keep"), Some(&Value::from(2)));
    assert_eq!(kept[0].pointer("/response/error_code"), None);
    assert_eq!(messages(&filter.handle_frame(&response("worker", "r1"))).len(), 1);
    assert_eq!(filter.issued_request_ids().count(), 0);
    assert_eq!(filter.expected_request_ids().collect::<Vec<_>>(), ["r2"]);
    assert_eq!(messages(&filter.handle_frame(&response("worker", "r2"))).len(), 1);
    assert_eq!(filter.expected_request_ids().count(), 0);
    let malformed = client(r#"{"payload":{"type":"control_response","response":{"request_id":5}}}"#);
    assert!(messages(&filter.handle_frame(&malformed)).is_empty());
    assert_eq!(dropped(&filter, SseDropCategory::ForgedReply), 2);
    assert_eq!(dropped(&filter, SseDropCategory::MalformedControlResponse), 1);
}

#[test]
fn non_worker_frames_of_worker_types_are_dropped() {
    let mut filter = SseFrameFilter::new(None);
    assert!(
        filter
            .prepare_write(r#"{"type":"control_request","request_id":"r1"}"#, || "u".to_owned())
            .is_ok()
    );
    for data in [
        r#"{"source":"client","payload":{"type":"control_request","request_id":"r1"}}"#,
        r#"{"source":"client","payload":{"type":"control_cancel_request","request_id":"zz"}}"#,
        r#"{"source":"client","payload":{"type":"assistant"}}"#,
        r#"{"payload":{"type":"result"}}"#,
    ] {
        assert!(messages(&filter.handle_frame(&client(data))).is_empty());
    }
    assert_eq!(dropped(&filter, SseDropCategory::ExpectedEcho), 1);
    assert_eq!(dropped(&filter, SseDropCategory::NonWorkerType), 3);
    let allowed = client(r#"{"source":"client","payload":{"type":"env_manager_log"}}"#);
    assert_eq!(messages(&filter.handle_frame(&allowed)).len(), 1);
}

#[test]
fn writes_get_a_uuid_and_track_control_requests() {
    let mut filter = SseFrameFilter::new(None);
    let write = filter.prepare_write(r#"{"type":"control_request","request_id":"r1"}"#, || "u".to_owned());
    assert_eq!(
        write.map(|write| write.line).ok().as_deref(),
        Some(r#"{"type":"control_request","request_id":"r1","uuid":"u"}"#)
    );
    assert_eq!(filter.issued_request_ids().collect::<Vec<_>>(), ["r1"]);
    assert!(
        filter
            .prepare_write(r#"{"type":"control_request","request_id":5}"#, || "u".to_owned())
            .is_ok()
    );
    assert!(filter.prepare_write("[1]", || "u".to_owned()).is_ok());
    assert!(filter.prepare_write("not json", || "u".to_owned()).is_err());
    assert_eq!(filter.issued_request_ids().count(), 1);
}

#[test]
fn the_expected_set_warns_when_it_evicts_an_unanswered_id() {
    let mut filter = SseFrameFilter::new(None);
    let warnings = (0..=EXPECTED_CAP)
        .filter_map(|n| filter.expect_control_response(&format!("q{n}")))
        .count();
    assert_eq!(warnings, 1);
    assert_eq!(filter.expected_request_ids().count(), EXPECTED_CAP);
}
