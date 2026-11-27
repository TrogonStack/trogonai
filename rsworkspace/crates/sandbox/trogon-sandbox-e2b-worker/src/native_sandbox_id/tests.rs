use super::*;
use crate::constants::MAX_NATIVE_SANDBOX_ID_BYTES;

#[test]
fn accepts_canonical_identities() -> Result<(), Box<dyn std::error::Error>> {
    let id = NativeSandboxId::new("ab-CD_09")?;
    assert_eq!(id.as_str(), "ab-CD_09");
    assert_eq!(String::from(id), "ab-CD_09");
    Ok(())
}

#[test]
fn refuses_empty_oversized_and_foreign_characters() {
    assert!(NativeSandboxId::new("").is_err());
    assert!(NativeSandboxId::new("a".repeat(MAX_NATIVE_SANDBOX_ID_BYTES + 1)).is_err());
    assert!(NativeSandboxId::new("a".repeat(MAX_NATIVE_SANDBOX_ID_BYTES)).is_ok());
    assert!(NativeSandboxId::new("a/b").is_err());
    assert!(NativeSandboxId::new("a b").is_err());
}

#[test]
fn deserialization_enforces_the_same_rule() -> Result<(), Box<dyn std::error::Error>> {
    let id: NativeSandboxId = serde_json::from_str("\"abc\"")?;
    assert_eq!(serde_json::to_string(&id)?, "\"abc\"");
    assert!(serde_json::from_str::<NativeSandboxId>("\"a/b\"").is_err());
    Ok(())
}
