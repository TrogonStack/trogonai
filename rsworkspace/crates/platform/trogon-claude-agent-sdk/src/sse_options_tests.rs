use super::*;

#[test]
fn debug_hides_urls_and_header_values() {
    let mut options = SseOptions::new("https://stream.example/s", "https://send.example/w", "session-1");
    options
        .headers
        .insert("Authorization".to_owned(), "Bearer secret".to_owned());
    options.on_catch_up_truncated = Some(Arc::new(|| {}));
    let text = format!("{options:?}");
    assert!(text.contains("Authorization"));
    assert!(text.contains("on_catch_up_truncated: true"));
    assert!(text.contains("on_delivery_update: false"));
    for hidden in ["secret", "stream.example", "send.example", "session-1"] {
        assert!(!text.contains(hidden), "{hidden} leaked into {text}");
    }
}
