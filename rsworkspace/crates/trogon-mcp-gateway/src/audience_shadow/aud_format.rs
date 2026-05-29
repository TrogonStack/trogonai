//! Expected mesh audience URI per ADR 0005 (`urn:trogon:mcp:backend:{tenant}:{server_id}`).

use super::errors::AudienceShadowError;

const BACKEND_AUD_PREFIX: &str = "urn:trogon:mcp:backend:";

fn validate_segment(value: &str, err: AudienceShadowError) -> Result<(), AudienceShadowError> {
    if value.is_empty() || value.contains(':') {
        return Err(err);
    }
    Ok(())
}

/// Compute the expected backend audience URI for the current hop.
///
/// # Errors
///
/// Returns [`AudienceShadowError::MalformedTenant`] or [`AudienceShadowError::MalformedServerId`]
/// when either segment is empty or contains `:` (which would break the URN layout).
pub fn compute_expected_aud(tenant: &str, server_id: &str) -> Result<String, AudienceShadowError> {
    validate_segment(tenant, AudienceShadowError::MalformedTenant)?;
    validate_segment(server_id, AudienceShadowError::MalformedServerId)?;
    Ok(format!("{BACKEND_AUD_PREFIX}{tenant}:{server_id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_valid_inputs() {
        assert_eq!(
            compute_expected_aud("acme", "github").expect("valid"),
            "urn:trogon:mcp:backend:acme:github"
        );
        assert_eq!(
            compute_expected_aud("tenant-1", "srv_2").expect("valid"),
            "urn:trogon:mcp:backend:tenant-1:srv_2"
        );
    }

    #[test]
    fn rejects_empty_tenant() {
        assert_eq!(
            compute_expected_aud("", "github"),
            Err(AudienceShadowError::MalformedTenant)
        );
    }

    #[test]
    fn rejects_empty_server_id() {
        assert_eq!(
            compute_expected_aud("acme", ""),
            Err(AudienceShadowError::MalformedServerId)
        );
    }

    #[test]
    fn rejects_tenant_with_colon() {
        assert_eq!(
            compute_expected_aud("acme:evil", "github"),
            Err(AudienceShadowError::MalformedTenant)
        );
    }

    #[test]
    fn rejects_server_id_with_colon() {
        assert_eq!(
            compute_expected_aud("acme", "git:hub"),
            Err(AudienceShadowError::MalformedServerId)
        );
    }
}
