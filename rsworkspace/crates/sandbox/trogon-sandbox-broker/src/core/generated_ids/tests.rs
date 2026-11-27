use std::error::Error;

use super::{
    generated_attempt_id, generated_correlation_id, generated_lease_id, generated_operation_id, lease_id_from_bytes,
    lease_id_from_parts,
};

#[test]
fn generated_ids_are_distinct_and_valid_tokens() -> Result<(), Box<dyn Error>> {
    assert_ne!(generated_lease_id(), generated_lease_id());
    assert_ne!(generated_correlation_id(), generated_correlation_id());
    assert_ne!(generated_operation_id()?, generated_operation_id()?);
    assert_ne!(generated_attempt_id()?, generated_attempt_id()?);
    Ok(())
}

#[test]
fn bytes_map_to_a_stable_26_character_id() {
    let first = lease_id_from_bytes([7; 16]);
    assert_eq!(first, lease_id_from_bytes([7; 16]));
    assert_ne!(first, lease_id_from_bytes([8; 16]));
    assert_eq!(first.as_str().len(), 26);
}

#[test]
fn parts_derive_a_stable_id_that_depends_on_every_part() {
    let base = lease_id_from_parts(["acme", "create", "op"]);
    assert_eq!(base, lease_id_from_parts(["acme", "create", "op"]));
    assert_ne!(base, lease_id_from_parts(["acme", "create", "other"]));
    assert_ne!(base, lease_id_from_parts(["acmecreate", "", "op"]));
}
