use super::*;
use jsonwebtoken::decode;

fn test_mint_request<'a>(
    subject: &'a SandboxId,
    tenant_id: &'a TenantId,
    lease_id: &'a LeaseId,
    audience: &'a str,
) -> MintRequest<'a> {
    MintRequest {
        issuer: "trogon-sandbox-egress-proxy",
        subject,
        tenant_id,
        lease_id,
        audience,
        ttl_seconds: 60,
    }
}

#[test]
fn minted_token_decodes_with_matching_claims() -> Result<(), Box<dyn std::error::Error>> {
    let signer = OidcSigner::generate()?;
    let tenant_id = TenantId::new("tenant-1")?;
    let lease_id = LeaseId::new("lease-1");
    let subject = SandboxId::new("sandbox-abc123")?;
    let audience = "https://forward.example.com/hook";

    let token = signer.mint(test_mint_request(&subject, &tenant_id, &lease_id, audience))?;

    let decoding_key = signer.decoding_key();
    let validation = validation_for_audience(audience);
    let decoded = decode::<OidcClaims>(token.expose_secret(), &decoding_key, &validation)?;

    assert_eq!(decoded.claims.tenant_id, tenant_id);
    assert_eq!(decoded.claims.lease_id, lease_id);
    assert_eq!(decoded.claims.aud, audience);
    assert_eq!(decoded.claims.sub, subject);
    assert!(decoded.claims.exp > decoded.claims.iat);
    assert_eq!(decoded.claims.exp - decoded.claims.iat, 60);
    Ok(())
}

#[test]
fn token_debug_never_reveals_raw_value() -> Result<(), Box<dyn std::error::Error>> {
    let signer = OidcSigner::generate()?;
    let tenant_id = TenantId::new("tenant-1")?;
    let lease_id = LeaseId::new("lease-1");
    let subject = SandboxId::new("sandbox-abc123")?;
    let token = signer.mint(test_mint_request(
        &subject,
        &tenant_id,
        &lease_id,
        "https://x.example.com",
    ))?;

    let debug_output = format!("{token:?}");
    assert_eq!(debug_output, "OidcToken([REDACTED])");
    assert!(!debug_output.contains(token.expose_secret()));
    Ok(())
}

#[test]
fn decoding_with_wrong_audience_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let signer = OidcSigner::generate()?;
    let tenant_id = TenantId::new("tenant-1")?;
    let lease_id = LeaseId::new("lease-1");
    let subject = SandboxId::new("sandbox-abc123")?;
    let token = signer.mint(test_mint_request(
        &subject,
        &tenant_id,
        &lease_id,
        "https://correct.example.com",
    ))?;

    let decoding_key = signer.decoding_key();
    let validation = validation_for_audience("https://wrong.example.com");
    let result = decode::<OidcClaims>(token.expose_secret(), &decoding_key, &validation);
    assert!(result.is_err(), "expected audience mismatch to be rejected");
    Ok(())
}

#[test]
fn decoding_with_untrusted_key_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let signer = OidcSigner::generate()?;
    let other_signer = OidcSigner::generate()?;
    let tenant_id = TenantId::new("tenant-1")?;
    let lease_id = LeaseId::new("lease-1");
    let subject = SandboxId::new("sandbox-abc123")?;
    let audience = "https://forward.example.com";
    let token = signer.mint(test_mint_request(&subject, &tenant_id, &lease_id, audience))?;

    let wrong_decoding_key = other_signer.decoding_key();
    let validation = validation_for_audience(audience);
    let result = decode::<OidcClaims>(token.expose_secret(), &wrong_decoding_key, &validation);
    assert!(
        result.is_err(),
        "expected signature verification against the wrong key to fail"
    );
    Ok(())
}

#[test]
fn from_ec_pkcs8_pem_round_trips_generated_key() -> Result<(), Box<dyn std::error::Error>> {
    let key_pair = KeyPair::generate()?;
    let pem = key_pair.serialize_pem();
    let signer = OidcSigner::from_ec_pkcs8_pem(&pem)?;

    let tenant_id = TenantId::new("tenant-1")?;
    let lease_id = LeaseId::new("lease-1");
    let subject = SandboxId::new("sandbox-abc123")?;
    let audience = "https://forward.example.com";
    let token = signer.mint(test_mint_request(&subject, &tenant_id, &lease_id, audience))?;

    let decoding_key = signer.decoding_key();
    let validation = validation_for_audience(audience);
    let decoded = decode::<OidcClaims>(token.expose_secret(), &decoding_key, &validation)?;
    assert_eq!(decoded.claims.aud, audience);
    Ok(())
}
