use super::*;

#[test]
fn minted_schedule_ids_are_canonical_uuid_v7_values() {
    let id = mint_schedule_id();
    let id = id.to_string();
    assert_eq!(id.len(), 32);
    assert_eq!(id.as_bytes()[12], b'7');
}
