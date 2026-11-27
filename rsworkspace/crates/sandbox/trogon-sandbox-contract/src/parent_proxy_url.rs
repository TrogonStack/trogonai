use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::{Host, ParentProxyUrlError, Port, ProxyScheme};

/// The address of a parent proxy and the credentials to present to it.
///
/// Construction takes an `http` or `https` URL and also accepts a schemeless `host:port`, read as `http`. When the
/// URL carries userinfo it is percent-decoded and kept as a `Basic` authorization value. That value is a secret, so
/// `Debug` prints `[REDACTED]` for it and the type has no `Display`.
#[derive(Clone, Eq, PartialEq)]
pub struct ParentProxyUrl {
    scheme: ProxyScheme,
    host: Host,
    port: Port,
    authorization: Option<String>,
}

impl ParentProxyUrl {
    pub fn new(raw: &str) -> Result<Self, ParentProxyUrlError> {
        let invalid = || ParentProxyUrlError::Invalid(redact_userinfo(raw));
        let raw = raw.trim();
        let with_scheme = if raw.contains("://") {
            raw.to_string()
        } else {
            format!("http://{raw}")
        };
        let url = url::Url::parse(&with_scheme).map_err(|_| invalid())?;
        let scheme = match url.scheme() {
            "http" => ProxyScheme::Http,
            "https" => ProxyScheme::Https,
            _ => return Err(ParentProxyUrlError::Scheme),
        };
        let host = url
            .host_str()
            .ok_or_else(invalid)
            .and_then(|host| Host::new(host).map_err(|_| invalid()))?;
        let port = url
            .port_or_known_default()
            .ok_or_else(invalid)
            .and_then(|port| Port::new(port).map_err(|_| invalid()))?;
        let authorization = (!url.username().is_empty() || url.password().is_some()).then(|| {
            let user = decode_userinfo(url.username());
            let password = decode_userinfo(url.password().unwrap_or_default());
            format!("Basic {}", STANDARD.encode(format!("{user}:{password}")))
        });
        Ok(Self {
            scheme,
            host,
            port,
            authorization,
        })
    }

    pub fn scheme(&self) -> ProxyScheme {
        self.scheme
    }

    pub fn host(&self) -> &Host {
        &self.host
    }

    pub fn port(&self) -> Port {
        self.port
    }

    /// The `Proxy-Authorization` header value, when the URL carried userinfo.
    pub fn authorization(&self) -> Option<&str> {
        self.authorization.as_deref()
    }
}

impl fmt::Debug for ParentProxyUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ParentProxyUrl")
            .field("scheme", &self.scheme)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("authorization", &self.authorization.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

/// Decodes the percent escapes of a userinfo part, replacing invalid UTF-8 and keeping `+` literal.
fn decode_userinfo(part: &str) -> String {
    let escaped = part.replace('+', "%2B").replace('&', "%26").replace('=', "%3D");
    url::form_urlencoded::parse(format!("userinfo={escaped}").as_bytes())
        .next()
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default()
}

/// Replaces the userinfo of a URL written as `scheme://user:secret@rest` with `***:***`.
fn redact_userinfo(raw: &str) -> String {
    match raw.split_once("//") {
        Some((scheme, rest)) => match rest.split_once('@') {
            Some((_, tail)) if !rest.starts_with('/') => format!("{scheme}//***:***@{tail}"),
            _ => raw.to_string(),
        },
        None => raw.to_string(),
    }
}
