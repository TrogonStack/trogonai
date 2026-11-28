use super::*;

fn aad(tenant: &str, credential_id: &str) -> Result<Vec<u8>, trogon_sandbox_contract::TenantIdError> {
    let tenant = TenantId::new(tenant)?;
    let credential_id = CredentialId::new(credential_id);
    Ok(RecordBinding {
        provider: CredentialProvider::Registry,
        tenant: &tenant,
        credential_id: &credential_id,
        version: 1,
    }
    .data_aad())
}

#[test]
fn aad_fields_cannot_shift() -> Result<(), Box<dyn std::error::Error>> {
    assert_ne!(aad("ab", "c")?, aad("a", "bc")?);
    Ok(())
}
