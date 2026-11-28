use trogon_sandbox_contract::{LeaseId, TenantId};
use trogon_sandbox_ws_gateway::{PreviewClaims, PreviewKeys, PreviewTokenError};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn claims() -> Result<PreviewClaims, Box<dyn std::error::Error>> {
    Ok(PreviewClaims {
        tenant: TenantId::new("tenant-test")?,
        lease_id: LeaseId::new("lease-1"),
        port: 3000,
        expires_at: 2_000,
    })
}

#[test]
fn rotation_keeps_old_tokens_valid_and_signs_with_the_active_key() -> TestResult {
    let old = PreviewKeys::parse(&format!("k1={}", "11".repeat(32)), None)?;
    let token = old.issue(&claims()?)?;

    let rotated = PreviewKeys::parse(&format!("k1={},k2={}", "11".repeat(32), "22".repeat(32)), Some("k2"))?;
    assert_eq!(rotated.verify(&token, 1_000)?, claims()?);
    assert!(rotated.issue(&claims()?)?.starts_with("v1.k2."));

    let dropped = PreviewKeys::parse(&format!("k2={}", "22".repeat(32)), None)?;
    assert!(matches!(
        dropped.verify(&token, 1_000),
        Err(PreviewTokenError::UnknownKey)
    ));
    Ok(())
}
