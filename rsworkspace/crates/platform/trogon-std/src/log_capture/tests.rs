use super::*;

#[test]
fn records_a_value_passed_as_a_field_under_its_field_name() {
    let events = CapturedEvents::new();
    let guard = events.install(LevelFilter::INFO);

    tracing::info!(user_id = 7, "request handled");
    drop(guard);

    let captured = events.events();
    let [event] = captured.as_slice() else {
        panic!("expected exactly one event, got {captured:?}");
    };
    assert_eq!(event.message(), Some("request handled"));
    assert_eq!(event.field("user_id"), Some("7"));
}

#[test]
fn records_a_string_field_without_the_quotes_debug_would_add() {
    let events = CapturedEvents::new();
    let guard = events.install(LevelFilter::INFO);

    tracing::info!(path = "/tmp/log", "request handled");
    drop(guard);

    let captured = events.events();
    let [event] = captured.as_slice() else {
        panic!("expected exactly one event, got {captured:?}");
    };
    assert_eq!(event.field("path"), Some("/tmp/log"));
}

#[test]
fn reports_a_value_left_in_the_message_as_a_missing_field() {
    let events = CapturedEvents::new();
    let guard = events.install(LevelFilter::INFO);

    let user_id = 7;
    tracing::info!("request handled for {user_id}");
    drop(guard);

    let captured = events.events();
    let [event] = captured.as_slice() else {
        panic!("expected exactly one event, got {captured:?}");
    };
    assert_eq!(event.message(), Some("request handled for 7"));
    assert_eq!(event.field("user_id"), None);
}

#[test]
fn skips_events_below_the_installed_level() {
    let events = CapturedEvents::new();
    let guard = events.install(LevelFilter::INFO);

    tracing::debug!(user_id = 7, "request handled");
    drop(guard);

    assert_eq!(events.events(), Vec::new());
}

#[test]
fn keeps_events_in_the_order_they_were_emitted() {
    let events = CapturedEvents::new();
    let guard = events.install(LevelFilter::TRACE);

    tracing::info!("first");
    tracing::trace!("second");
    drop(guard);

    let captured = events.events();
    let messages: Vec<_> = captured.iter().map(CapturedEvent::message).collect();
    assert_eq!(messages, vec![Some("first"), Some("second")]);
}

#[test]
fn debug_renders_the_captured_fields() {
    let events = CapturedEvents::new();
    let guard = events.install(LevelFilter::INFO);

    tracing::info!(user_id = 7, "request handled");
    drop(guard);

    let rendered = format!("{:?}", events.events());
    assert!(rendered.contains("user_id"), "{rendered}");
}
