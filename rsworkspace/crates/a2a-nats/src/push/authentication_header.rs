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
mod tests {
    use super::*;

    fn auth(scheme: &str, credentials: &str) -> AuthenticationInfo {
        AuthenticationInfo {
            scheme: scheme.to_string(),
            credentials: Some(credentials.to_string()),
        }
    }

    #[test]
    fn bearer_normalized() {
        let v = authorization_header_value(Some(&auth("Bearer", "tok")))
            .unwrap()
            .unwrap();
        assert_eq!(v, "Bearer tok");
        let v2 = authorization_header_value(Some(&auth("jwt", "signed.jwt.here")))
            .unwrap()
            .unwrap();
        assert_eq!(v2, "Bearer signed.jwt.here");
    }

    #[test]
    fn basic_passes_through_base64_token() {
        let v = authorization_header_value(Some(&auth("basic", "dXNlcjpwdw==")))
            .unwrap()
            .unwrap();
        assert_eq!(v, "Basic dXNlcjpwdw==");
    }

    #[test]
    fn custom_scheme_formats_token_pair() {
        let v = authorization_header_value(Some(&auth("BearerToken", "abc")))
            .unwrap()
            .unwrap();
        assert_eq!(v, "BearerToken abc");
    }

    #[test]
    fn digest_errors() {
        let err = authorization_header_value(Some(&auth("digest", "opaque"))).unwrap_err();
        assert!(matches!(err, AuthenticationHeaderBuildError::UnsupportedScheme { .. }));
    }

    #[test]
    fn negotiate_and_ntlm_are_rejected_as_unsupported_challenge_schemes() {
        for scheme in ["Negotiate", "ntlm", "NTLM"] {
            let err = authorization_header_value(Some(&auth(scheme, "tok"))).unwrap_err();
            assert!(
                matches!(err, AuthenticationHeaderBuildError::UnsupportedScheme { .. }),
                "{scheme} must round-trip through the unsupported-challenge arm"
            );
        }
    }

    #[test]
    fn none_absent_when_no_authentication_msg() {
        assert!(authorization_header_value(None).unwrap().is_none());
    }

    #[test]
    fn missing_scheme_is_rejected() {
        let err = authorization_header_value(Some(&auth("   ", "tok"))).unwrap_err();
        assert!(matches!(err, AuthenticationHeaderBuildError::MissingScheme));
    }

    #[test]
    fn bearer_without_credentials_is_rejected() {
        let err = authorization_header_value(Some(&auth("Bearer", "   "))).unwrap_err();
        assert!(matches!(err, AuthenticationHeaderBuildError::EmptyCredentials));
    }

    #[test]
    fn basic_without_credentials_is_rejected() {
        let err = authorization_header_value(Some(&auth("Basic", ""))).unwrap_err();
        assert!(matches!(err, AuthenticationHeaderBuildError::EmptyCredentials));
    }

    #[test]
    fn custom_scheme_without_credentials_is_rejected() {
        let err = authorization_header_value(Some(&auth("CustomScheme", ""))).unwrap_err();
        assert!(matches!(err, AuthenticationHeaderBuildError::EmptyCredentials));
    }

    #[test]
    fn error_display_covers_every_variant() {
        assert!(
            AuthenticationHeaderBuildError::MissingScheme
                .to_string()
                .contains("must not be empty")
        );
        assert!(
            AuthenticationHeaderBuildError::EmptyCredentials
                .to_string()
                .contains("credentials required")
        );
        assert!(
            AuthenticationHeaderBuildError::UnsupportedScheme {
                scheme: "Digest".into()
            }
            .to_string()
            .contains("not supported yet")
        );
    }
}
