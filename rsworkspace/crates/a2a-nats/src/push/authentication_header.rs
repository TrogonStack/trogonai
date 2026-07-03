//! Build `Authorization` header values from A2A [`AuthenticationInfo`] (HTTPS push targets).
//!
//! Digest / mutual challenge schemes are intentionally rejected until a dedicated signing path exists.

use a2a::types::AuthenticationInfo;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthenticationHeaderBuildError {
    #[error("push authentication.scheme must not be empty")]
    MissingScheme,
    #[error("push authentication.credentials required for scheme")]
    EmptyCredentials,
    /// `Digest` (and challenge-based schemes) require a separate implementation.
    #[error("push authentication scheme {scheme:?} not supported yet")]
    UnsupportedScheme { scheme: String },
}

/// Builds the RFC 9110 [`Authorization`] field value (`<scheme> <token>` for typical schemes).
pub fn authorization_header_value(
    authentication: Option<&AuthenticationInfo>,
) -> Result<Option<String>, AuthenticationHeaderBuildError> {
    let Some(auth) = authentication else {
        return Ok(None);
    };

    let scheme_raw = auth.scheme.trim();
    if scheme_raw.is_empty() {
        return Err(AuthenticationHeaderBuildError::MissingScheme);
    }

    let credentials = auth.credentials.as_deref().unwrap_or("").trim();
    let scheme_lc = scheme_raw.to_ascii_lowercase();

    match scheme_lc.as_str() {
        // Challenge-based schemes (RFC 7235) need a dedicated signing path; rejecting
        // every one we know of up front keeps the static "scheme creds" fallback
        // from silently producing an unusable Authorization value.
        "digest" | "negotiate" | "ntlm" => Err(AuthenticationHeaderBuildError::UnsupportedScheme {
            scheme: auth.scheme.trim().to_string(),
        }),
        "bearer" | "jwt" => {
            if credentials.is_empty() {
                return Err(AuthenticationHeaderBuildError::EmptyCredentials);
            }
            Ok(Some(format!("Bearer {credentials}")))
        }
        "basic" => {
            if credentials.is_empty() {
                return Err(AuthenticationHeaderBuildError::EmptyCredentials);
            }
            // Per RFC 7617 the token after `Basic` is the base64(user:passwd) blob.
            Ok(Some(format!("Basic {credentials}")))
        }
        _ => {
            if credentials.is_empty() {
                return Err(AuthenticationHeaderBuildError::EmptyCredentials);
            }
            let scheme_trimmed = scheme_raw.to_owned();
            Ok(Some(format!("{scheme_trimmed} {credentials}")))
        }
    }
}

#[cfg(test)]
mod tests;
