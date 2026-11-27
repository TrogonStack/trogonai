use std::error::Error;

use trogon_sandbox_contract::SnapshotGeneration;

use super::{encode, generated_ulid};

#[test]
fn generated_ulid_is_a_canonical_snapshot_generation() -> Result<(), Box<dyn Error>> {
    let value = generated_ulid();
    assert_eq!(SnapshotGeneration::new(value.as_str())?.as_str(), value);
    Ok(())
}

#[test]
fn successive_ulids_differ() {
    assert_ne!(generated_ulid(), generated_ulid());
}

#[test]
fn encoding_covers_the_extremes() {
    assert_eq!(encode(0), "00000000000000000000000000");
    assert_eq!(encode(u128::MAX), "7ZZZZZZZZZZZZZZZZZZZZZZZZZ");
}
