use super::*;

#[test]
fn read_model_key_is_stable_and_distinct_per_schedule() {
    let key = read_model_key("orders/created");
    assert!(!key.is_empty());
    assert_eq!(key, read_model_key("orders/created"));
    assert_ne!(key, read_model_key("orders/updated"));
}
