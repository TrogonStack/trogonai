use std::fmt;

use secrecy::SecretString;
use trogon_sandbox_contract::{
    AwsCredentialPairBinding, AwsCredentialPairsError, Host, InjectHosts, InjectScope, ResponseHeaderStrip, SecretName,
};

use crate::StripRule;
use crate::registry_entry::RegistryEntry;
use crate::{AwsCredentialPair, RealValue, RequestSite, Sentinel};

/// Sentinel to real value map for one sandbox session.
///
/// Substitution is gated per sentinel: a sentinel is swapped only when the destination matches that binding's own
/// inject hosts, so one credential cannot be laundered through another binding's allowed host. It lives in memory
/// only.
#[derive(Default)]
pub struct SentinelRegistry {
    pub(crate) entries: Vec<RegistryEntry>,
    pub(crate) aws_pairs: Vec<AwsCredentialPairBinding>,
    pub(crate) strip_rules: Vec<StripRule>,
}

impl SentinelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the sentinel for `name`, minting one on first use.
    ///
    /// A repeat call keeps the existing sentinel, so a process that already read it stays valid, and updates the real
    /// value and inject hosts in place.
    pub fn register(&mut self, name: SecretName, real: RealValue, inject_hosts: InjectHosts) -> Sentinel {
        let sentinel = Sentinel::mint(&real);
        self.register_with_sentinel(name, sentinel, real, inject_hosts)
    }

    /// Registers `name` under `sentinel` unless the name is already registered, in which case the existing sentinel
    /// is kept and the real value and inject hosts are updated in place.
    pub(crate) fn register_with_sentinel(
        &mut self,
        name: SecretName,
        sentinel: Sentinel,
        real: RealValue,
        inject_hosts: InjectHosts,
    ) -> Sentinel {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.name == name) {
            entry.real = real;
            entry.inject_hosts = inject_hosts;
            return entry.sentinel.clone();
        }
        self.entries.push(RegistryEntry {
            name,
            sentinel: sentinel.clone(),
            real,
            inject_hosts,
            scope: InjectScope::default(),
        });
        sentinel
    }

    /// Applies `scope` to the entry `name` and to every entry derived from it.
    ///
    /// Derived entries are named `<name>.claim-<n>` and `<name>.capture-<n>`. Call it after `register` and
    /// `register_jwt`. Entries registered later through `register_captures` inherit the scope of the binding they
    /// come from.
    pub fn set_scope(&mut self, name: &SecretName, scope: &InjectScope) {
        let derived = format!("{name}.");
        for entry in self
            .entries
            .iter_mut()
            .filter(|entry| &entry.name == name || entry.name.as_str().starts_with(&derived))
        {
            entry.scope = scope.clone();
        }
    }

    /// Links already registered bindings into an AWS credential pair.
    ///
    /// A repeat call for the same access key id binding replaces the earlier link.
    pub fn link_aws_pair(&mut self, pair: &AwsCredentialPairBinding) -> Result<(), AwsCredentialPairsError> {
        for member in pair.members() {
            if self.entry(member).is_none() {
                return Err(AwsCredentialPairsError::UnknownBinding(member.clone()));
            }
        }
        match self
            .aws_pairs
            .iter_mut()
            .find(|linked| linked.access_key_id() == pair.access_key_id())
        {
            Some(linked) => *linked = pair.clone(),
            None => self.aws_pairs.push(pair.clone()),
        }
        Ok(())
    }

    /// Linked AWS pairs whose access key id binding injects at `host:port`.
    pub fn aws_pairs_for_host<'a>(
        &'a self,
        host: &'a Host,
        port: u16,
    ) -> impl Iterator<Item = AwsCredentialPair<'a>> + 'a {
        self.aws_pairs_for_request(host, port, None)
    }

    /// [`Self::aws_pairs_for_host`] for one request.
    pub fn aws_pairs_for_request<'a>(
        &'a self,
        host: &'a Host,
        port: u16,
        site: Option<RequestSite<'a>>,
    ) -> impl Iterator<Item = AwsCredentialPair<'a>> + 'a {
        self.aws_pairs.iter().filter_map(move |linked| {
            let access_key_id = self.entry(linked.access_key_id())?;
            if !access_key_id.injects_at(host, port, site.as_ref()) {
                return None;
            }
            let secret_access_key = self.entry(linked.secret_access_key())?;
            let session_token = match linked.session_token() {
                Some(name) => Some(&self.entry(name)?.real),
                None => None,
            };
            Some(AwsCredentialPair {
                binding: &access_key_id.name,
                access_key_id_sentinel: &access_key_id.sentinel,
                access_key_id: &access_key_id.real,
                secret_access_key: &secret_access_key.real,
                session_token,
            })
        })
    }

    /// Strips `headers` from replies of `hosts`. A host named by several rules has the union stripped.
    pub fn register_strip(&mut self, hosts: InjectHosts, headers: ResponseHeaderStrip) {
        if headers.is_empty() {
            return;
        }
        self.strip_rules.push(StripRule { hosts, headers });
    }

    /// The union of the headers stripped from replies of `host:port`.
    pub fn strip_for_host(&self, host: &Host, port: u16) -> ResponseHeaderStrip {
        ResponseHeaderStrip::new(
            self.strip_rules
                .iter()
                .filter(|rule| rule.applies_at(host, port))
                .flat_map(|rule| rule.headers.headers().iter().cloned()),
        )
    }

    pub(crate) fn entry(&self, name: &SecretName) -> Option<&RegistryEntry> {
        self.entries.iter().find(|entry| &entry.name == name)
    }

    pub fn sentinel_for(&self, name: &SecretName) -> Option<&Sentinel> {
        self.entry(name).map(|entry| &entry.sentinel)
    }

    pub fn lookup_real(&self, sentinel: &Sentinel) -> Option<&RealValue> {
        self.entries
            .iter()
            .find(|entry| &entry.sentinel == sentinel)
            .map(|entry| &entry.real)
    }

    /// Bindings that name `host:port`, whatever the request is.
    ///
    /// It includes bindings scoped to a path or method, because the connection has to be terminated to find out
    /// whether a request is in scope.
    pub fn names_injectable_at(&self, host: &Host, port: u16) -> Vec<&SecretName> {
        self.entries
            .iter()
            .filter(|entry| entry.hosted_at(host, port))
            .map(|entry| &entry.name)
            .collect()
    }

    /// Bindings whose credential may be sent for this request.
    pub fn names_injectable_for(&self, host: &Host, port: u16, site: Option<RequestSite<'_>>) -> Vec<&SecretName> {
        self.entries
            .iter()
            .filter(|entry| entry.injects_at(host, port, site.as_ref()))
            .map(|entry| &entry.name)
            .collect()
    }

    /// Sentinel and real value of every binding that injects at `host:port` regardless of the request.
    ///
    /// A binding scoped to a path or method is not among them; use [`Self::pairs_for_request`].
    pub fn pairs_for_host<'a>(
        &'a self,
        host: &'a Host,
        port: u16,
    ) -> impl Iterator<Item = (&'a Sentinel, &'a RealValue)> + 'a {
        self.pairs_for_request(host, port, None)
    }

    /// [`Self::pairs_for_host`] for one request.
    pub fn pairs_for_request<'a>(
        &'a self,
        host: &'a Host,
        port: u16,
        site: Option<RequestSite<'a>>,
    ) -> impl Iterator<Item = (&'a Sentinel, &'a RealValue)> + 'a {
        self.entries
            .iter()
            .filter(move |entry| entry.injects_at(host, port, site.as_ref()))
            .map(|entry| (&entry.sentinel, &entry.real))
    }

    /// Every binding's sentinel and real value regardless of destination, for scrubbing real values out of what
    /// comes back from an upstream.
    pub fn pairs(&self) -> impl Iterator<Item = (&Sentinel, &RealValue)> + '_ {
        self.entries.iter().map(|entry| (&entry.sentinel, &entry.real))
    }

    /// The bindings whose credential must not be sent to `host:port`, whatever the request is.
    pub fn pairs_off_host<'a>(
        &'a self,
        host: &'a Host,
        port: u16,
    ) -> impl Iterator<Item = (&'a SecretName, &'a Sentinel)> + 'a {
        self.pairs_off_request(host, port, None)
    }

    /// [`Self::pairs_off_host`] for one request: the bindings whose credential must not be sent for it.
    pub fn pairs_off_request<'a>(
        &'a self,
        host: &'a Host,
        port: u16,
        site: Option<RequestSite<'a>>,
    ) -> impl Iterator<Item = (&'a SecretName, &'a Sentinel)> + 'a {
        self.entries
            .iter()
            .filter(move |entry| !entry.injects_at(host, port, site.as_ref()))
            .map(|entry| (&entry.name, &entry.sentinel))
    }

    /// Replaces every sentinel whose binding injects at `host:port` with its real value.
    ///
    /// Sentinels of other bindings are left as the useless fake.
    pub fn substitute(&self, input: &str, host: &Host, port: u16) -> SecretString {
        let mut out = input.to_string();
        for (sentinel, real) in self.pairs_for_host(host, port) {
            if out.contains(sentinel.as_str()) {
                out = out.replace(sentinel.as_str(), real.expose());
            }
        }
        SecretString::from(out)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.aws_pairs.clear();
        self.strip_rules.clear();
    }
}

impl fmt::Debug for SentinelRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_list()
            .entries(self.entries.iter().map(|entry| (&entry.name, &entry.sentinel)))
            .finish()
    }
}
