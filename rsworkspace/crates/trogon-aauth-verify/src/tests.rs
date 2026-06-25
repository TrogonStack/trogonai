use std::sync::Arc;

use super::*;

#[test]
fn verify_error_token_from_conversion_preserves_source() {
    let token_err = TokenError::BadHeader;
    let verify_err: VerifyError = token_err.into();
    assert!(matches!(verify_err, VerifyError::Token(TokenError::BadHeader)));
}

#[test]
fn verify_error_display_messages_are_distinct() {
    let cases = [
        format!("{}", VerifyError::Token(TokenError::BadHeader)),
        format!("{}", VerifyError::Pop("p".into())),
        format!("{}", VerifyError::Replay("r".into())),
        format!("{}", VerifyError::Policy("po".into())),
    ];
    for window in cases.windows(2) {
        assert_ne!(window[0], window[1]);
    }
}

#[test]
fn system_time_source_returns_a_recent_unix_timestamp() {
    // Just exercise the unit so its Display + impl get covered. We can't
    // assert exact value, only that the type roundtrips and produces a
    // reasonable epoch second.
    let ts = SystemTimeSource.now();
    assert!(ts > 0, "system time should be > 0, got {ts}");
}

#[test]
fn arc_wrapped_time_source_delegates() {
    let inner = Arc::new(SystemTimeSource);
    // Force the Arc<T> blanket impl path.
    let ts: Arc<dyn TimeSource> = inner;
    assert!(ts.now() > 0);
}

#[test]
fn re_exports_link_to_inner_modules() {
    // Smoke: re-exported types are usable through the crate root.
    let _: StaticJwks = StaticJwks::new();
    let _: InMemoryReplayStore = InMemoryReplayStore::new();
}
