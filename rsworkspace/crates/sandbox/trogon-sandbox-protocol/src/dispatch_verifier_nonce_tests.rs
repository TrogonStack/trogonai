use std::time::{Duration, UNIX_EPOCH};

use super::*;

#[test]
fn a_full_nonce_table_drops_only_expired_nonces() -> Result<(), Box<dyn std::error::Error>> {
    let verifier = DispatchVerifier::new(EnforcementMode::Require);
    let now = UNIX_EPOCH + Duration::from_secs(10_000);
    for index in 0..MAX_NONCES {
        let expires_at = if index < 10 { 100 } else { 20_000 };
        verifier.note_nonce(&format!("nonce-{index}"), expires_at, now)?;
    }
    verifier.note_nonce("fresh", 20_000, now)?;
    let seen = verifier.nonces.lock().map_err(|_| "nonce table poisoned")?;
    assert_eq!(seen.len(), MAX_NONCES - 10 + 1);
    assert!(!seen.contains_key("nonce-0"));
    assert!(seen.contains_key("nonce-10"));
    Ok(())
}
