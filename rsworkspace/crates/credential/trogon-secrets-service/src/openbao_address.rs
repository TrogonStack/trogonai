use trogon_nats::constants::TLS_DISABLED_VALUES;
use trogon_secret_store::OpenBaoTransport;
use url::Url;

use super::OpenBaoAddressError;

/// Where the service reaches OpenBao, and whether over TLS.
///
/// The address is `https` unless the TLS switch is set to one of the values
/// that disable TLS for NATS, and then it is `http`. Either way it carries no
/// credentials, so it is safe to log.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenBaoAddress {
    url: Url,
    transport: OpenBaoTransport,
}

impl OpenBaoAddress {
    /// Reads `address` with `tls_disabled`, the value of the switch that
    /// disables TLS, when it is set. An empty switch counts as unset, as an
    /// empty `NATS_TLS_DISABLED` does.
    ///
    /// # Errors
    ///
    /// Returns [`OpenBaoAddressError`] when the address is not a URL, carries
    /// credentials, or disagrees with the switch, and when the switch holds a
    /// value it does not know.
    pub fn new(address: &str, tls_disabled: Option<&str>) -> Result<Self, OpenBaoAddressError> {
        let url = Url::parse(address).map_err(OpenBaoAddressError::NotAUrl)?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err(OpenBaoAddressError::Credentials);
        }
        let disabled = match tls_disabled.filter(|value| !value.is_empty()) {
            None => false,
            Some(value)
                if TLS_DISABLED_VALUES
                    .iter()
                    .any(|known| value.eq_ignore_ascii_case(known)) =>
            {
                true
            }
            Some(value) => {
                return Err(OpenBaoAddressError::UnrecognizedTlsDisabledValue {
                    value: value.to_owned(),
                });
            }
        };
        let transport = match (url.scheme(), disabled) {
            ("https", false) => OpenBaoTransport::TlsRequired,
            ("http", true) => OpenBaoTransport::PlaintextAllowed,
            ("http", false) => return Err(OpenBaoAddressError::Plaintext),
            ("https", true) => return Err(OpenBaoAddressError::TlsDisabledForHttps),
            (scheme, _) => return Err(OpenBaoAddressError::UnsupportedScheme(scheme.to_owned())),
        };
        Ok(Self { url, transport })
    }

    /// The address, with the trailing slash a URL with no path reads with.
    pub fn as_str(&self) -> &str {
        self.url.as_str()
    }

    /// Whether the address is reached over TLS.
    pub fn transport(&self) -> OpenBaoTransport {
        self.transport
    }
}

#[cfg(test)]
mod tests;
