use trogon_sandbox_contract::{Host, InjectHosts, InjectScope, SecretName};

use crate::{RealValue, RequestSite, Sentinel};
use crate::{inject_host_matches, inject_scope_admits};

pub(crate) struct RegistryEntry {
    pub(crate) name: SecretName,
    pub(crate) sentinel: Sentinel,
    pub(crate) real: RealValue,
    pub(crate) inject_hosts: InjectHosts,
    pub(crate) scope: InjectScope,
}

impl RegistryEntry {
    /// Reports whether the destination host and port are one this entry names, whatever the request is.
    pub(crate) fn hosted_at(&self, host: &Host, port: u16) -> bool {
        self.inject_hosts
            .hosts()
            .iter()
            .any(|pattern| inject_host_matches(&self.scope, pattern, host, port))
    }

    /// Reports whether the entry's credential may be sent for this request.
    ///
    /// With no `site`, a connection-level decision made before any request is read, an entry scoped to a path or
    /// method is out of scope, so a caller that forgets the request fails closed.
    pub(crate) fn injects_at(&self, host: &Host, port: u16, site: Option<&RequestSite<'_>>) -> bool {
        if !self.hosted_at(host, port) {
            return false;
        }
        if self.scope.is_request_independent() {
            return true;
        }
        site.is_some_and(|site| inject_scope_admits(&self.scope, site.method, site.path))
    }
}
