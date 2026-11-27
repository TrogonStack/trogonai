use crate::{NoProxy, ParentProxyError, ParentProxyUrl};

/// The parent proxy the proxy chains through, and the hosts that skip it.
///
/// At least one of the `http` and `https` proxy URLs is present.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ParentProxy {
    http: Option<ParentProxyUrl>,
    https: Option<ParentProxyUrl>,
    no_proxy: NoProxy,
}

impl ParentProxy {
    pub fn new(
        http: Option<ParentProxyUrl>,
        https: Option<ParentProxyUrl>,
        no_proxy: NoProxy,
    ) -> Result<Self, ParentProxyError> {
        if http.is_none() && https.is_none() {
            return Err(ParentProxyError::NoProxyUrl);
        }
        Ok(Self { http, https, no_proxy })
    }

    pub fn http(&self) -> Option<&ParentProxyUrl> {
        self.http.as_ref()
    }

    pub fn https(&self) -> Option<&ParentProxyUrl> {
        self.https.as_ref()
    }

    pub fn no_proxy(&self) -> &NoProxy {
        &self.no_proxy
    }
}
