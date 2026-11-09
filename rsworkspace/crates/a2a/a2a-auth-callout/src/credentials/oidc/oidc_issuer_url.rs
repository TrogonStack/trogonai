use crate::error::{AuthCalloutError, CredentialError};

type UrlParseError = <reqwest::Url as std::str::FromStr>::Err;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcIssuerUrl(String);

#[derive(Debug, thiserror::Error)]
pub enum OidcIssuerUrlError {
    #[error("OIDC issuer URL is invalid")]
    InvalidUrl(#[source] UrlParseError),
    #[error("OIDC issuer requires HTTPS and a host")]
    HttpsRequired,
    #[error("OIDC issuer cannot contain credentials, query, fragment or whitespace")]
    UnsupportedComponents,
}

impl OidcIssuerUrl {
    pub fn parse(url: impl Into<String>) -> Result<Self, OidcIssuerUrlError> {
        let raw = url.into();
        let parsed = reqwest::Url::parse(&raw).map_err(OidcIssuerUrlError::InvalidUrl)?;
        if parsed.scheme() != "https" || parsed.host_str().is_none() {
            return Err(OidcIssuerUrlError::HttpsRequired);
        }
        if !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || raw.chars().any(char::is_whitespace)
        {
            return Err(OidcIssuerUrlError::UnsupportedComponents);
        }
        Ok(Self(raw))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[cfg(test)]
    pub(super) fn loopback_for_test(raw: impl Into<String>) -> Self {
        let raw = raw.into();
        let parsed = reqwest::Url::parse(&raw).unwrap();
        assert_eq!(parsed.scheme(), "http");
        assert!(
            parsed
                .host_str()
                .unwrap()
                .parse::<std::net::IpAddr>()
                .unwrap()
                .is_loopback()
        );
        Self(raw)
    }
}

impl From<OidcIssuerUrlError> for AuthCalloutError {
    fn from(error: OidcIssuerUrlError) -> Self {
        CredentialError::OidcIssuer(error).into()
    }
}
