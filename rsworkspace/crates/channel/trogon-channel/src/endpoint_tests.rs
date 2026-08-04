use super::*;

#[test]
fn endpoint_accepts_negative_telegram_chat_ids() {
    let e = Endpoint::new("telegram", "mybot", "-1001234567890").expect("valid");
    assert_eq!(e.kv_key(), "telegram.mybot.-1001234567890");
}

#[test]
fn endpoint_rejects_unsafe_tokens() {
    assert!(Endpoint::new("telegram", "my bot", "1").is_err());
    assert!(Endpoint::new("", "mybot", "1").is_err());
    assert!(Endpoint::new("telegram", "mybot", "a.b").is_err());
}
