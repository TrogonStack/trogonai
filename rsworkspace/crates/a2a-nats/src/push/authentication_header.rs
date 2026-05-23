//! Build `Authorization` header values from A2A [`AuthenticationInfo`] (HTTPS push targets).
//!
//! Digest / mutual challenge schemes are intentionally rejected until a dedicated signing path exists.

use std::fmt;

use a2a_types::AuthenticationInfo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthenticationHeaderBuildError {
    MissingScheme,
    EmptyCredentials,
    /// `Digest` (and challenge-based schemes) require a separate implementation.
    UnsupportedScheme {
        scheme: String,
    },
}

impl fmt::Display for AuthenticationHeaderBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingScheme => f.write_str("push authentication.scheme must not be empty"),
            Self::EmptyCredentials => f.write_str("push authentication.credentials required for scheme"),
            Self::UnsupportedScheme { scheme } => {
                write!(f, "push authentication scheme {scheme:?} not supported yet")
            }
        }
    }
}

impl std::error::Error for AuthenticationHeaderBuildError {}

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

    let credentials = auth.credentials.trim();
    let scheme_lc = scheme_raw.to_ascii_lowercase();

    match scheme_lc.as_str() {
        "digest" => Err(AuthenticationHeaderBuildError::UnsupportedScheme {
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
            credentials: credentials.to_string(),
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
    fn none_absent_when_no_authentication_msg() {
        assert!(authorization_header_value(None).unwrap().is_none());
    }
}
