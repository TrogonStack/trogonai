use super::*;
use trogon_std::UuidV7Generator;

#[test]
fn generated_ids_are_v7_in_simple_form() {
    let id = ConversationId::generate(&UuidV7Generator);
    assert_eq!(id.as_str().len(), 32);
    assert!(id.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(id.as_str().chars().nth(12), Some('7'), "version nibble");
}

#[test]
fn generated_ids_sort_in_creation_order() {
    let first = ConversationId::generate(&UuidV7Generator);
    let second = ConversationId::generate(&UuidV7Generator);
    assert!(first.as_str() < second.as_str());
}
