//! The settings document (`~/.sandbox-settings.json`), read into a
//! [`PolicyExecConfig`].
//!
//! Options this runtime cannot enforce are rejected with the path of the
//! option, never accepted and ignored: a settings file that asks for a rule
//! must not run without it. A key nobody defined is rejected too, unless the
//! caller asks for the lenient reading, which drops it.

use std::collections::BTreeMap;
use std::fmt;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use ipnet::IpNet;
use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use trogon_sandbox_contract::Port;
use trogon_sandbox_contract::{
    AllowedTlsTermination, CaFiles, DeniedDomainReasons, DenyReason, ExternalProxy, ExtraCaCertPaths, LoopbackNames,
    MitmUnixSocket, NamesMatchedByName, NetworkOptions, NoProxy, ParentProxy, ParentProxyUrl, PatternTarget,
    PlaintextHeaderSet, PrivateRangesByName, ReasonedRefusals, TlsExclusions, TunnelCap, UnlistedDestinations,
};
use trogon_sandbox_contract::{
    DeniedDomain, DomainPattern, FsPathEntry, FsPolicy, GlobPattern, LiteralPath, MachServiceName,
    MandatoryDenySearchDepth, PlatformOptions, ProtectedNamesAnchor, ResponseHeaderStrip, StrippedHeader,
};
use trogon_sandbox_contract::{DomainRule, DomainRuleAction, IpRange, NetworkPolicy, OpaqueTunnelPolicy};
use trogon_sandbox_egress_proxy::IgnoreViolations;
use trogon_sandbox_protocol::HostMatch;
use trogon_std::env::{ReadEnv, SystemEnv};

use self::fs_path::FsPathError;
use crate::config::{FilesystemView, PolicyExecConfig, RuntimePolicy};

use benign::{
    AmbiguousTarget, HandshakeTimeoutMs, HostMatch as HostMatchSetting, RipgrepSettings, SearchDepth, SeccompSettings,
};
pub use host_paths::HostPaths;
use host_paths::{ListKind, ResolvedEntry, has_glob_chars, remove_trailing_glob_suffix};

mod benign;
mod constants;
mod domain_rules;
mod fs_path;
mod host_paths;
mod issue_wording;
mod issues;
mod refine;
#[cfg(any(windows, test))]
pub(crate) mod windows_paths;
mod windows_section;

pub use self::credentials::CredentialOverride;
use self::credentials::CredentialSettings;
pub(crate) use self::credentials::{CredentialContext, EffectiveCredentials};

mod credentials;

/// The `windows` section of the settings: where trogon-sandbox-fence-helper is, and which fence
/// the install made.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct WindowsSettings {
    pub sandbox_user: Option<trogon_sandbox_windows_fence::SandboxUser>,
    pub sublayer_guid: Option<trogon_sandbox_windows_fence::SublayerGuid>,
    pub proxy_port_range: Option<trogon_sandbox_windows_fence::ProxyPortRange>,
    pub fence_helper_path: Option<PathBuf>,
}

impl WindowsSettings {
    fn read(section: Option<&windows_section::WindowsSection>) -> Result<Self, SettingsError> {
        let Some(section) = section else {
            return Ok(Self::default());
        };
        let invalid = |key: &str, why: String| SettingsError::new(format!("windows.{key}"), why);
        let sandbox_user = section
            .sandbox_user
            .as_deref()
            .map(trogon_sandbox_windows_fence::SandboxUser::new)
            .transpose()
            .map_err(|why| invalid("sandboxUser", why.to_string()))?;
        let sublayer_guid = section
            .sublayer_guid
            .as_deref()
            .or(section.wfp_sublayer_guid.as_deref())
            .map(trogon_sandbox_windows_fence::SublayerGuid::new)
            .transpose()
            .map_err(|why| invalid("sublayerGuid", why.to_string()))?;
        let proxy_port_range = section
            .proxy_port_range
            .map(|(low, high)| trogon_sandbox_windows_fence::ProxyPortRange::new(low, high))
            .transpose()
            .map_err(|why| invalid("proxyPortRange", why.to_string()))?;
        let fence_helper_path = section
            .fence_helper
            .as_ref()
            .map(|fence_helper| PathBuf::from(&fence_helper.path));
        Ok(Self {
            sandbox_user,
            sublayer_guid,
            proxy_port_range,
            fence_helper_path,
        })
    }
}

/// An optional key that is absent or has a value; `null` is refused.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

struct ObjectOnly<T>(T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for ObjectOnly<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Only<T>(std::marker::PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for Only<T> {
            type Value = T;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a map")
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<T, A::Error> {
                T::deserialize(de::value::MapAccessDeserializer::new(map))
            }
        }

        deserializer.deserialize_map(Only(std::marker::PhantomData)).map(Self)
    }
}

fn object<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    ObjectOnly::<T>::deserialize(deserializer).map(|only| only.0)
}

fn object_present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    object(deserializer).map(Some)
}

fn objects<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Vec::<ObjectOnly<T>>::deserialize(deserializer)?
        .into_iter()
        .map(|only| only.0)
        .collect())
}

fn mach_services<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<MachServiceName>, D::Error> {
    struct Name(Option<MachServiceName>);

    impl<'de> Deserialize<'de> for Name {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let raw = String::deserialize(deserializer)?;
            let stem = raw.strip_suffix('*').unwrap_or(&raw);
            if stem.contains('*') {
                return Err(de::Error::custom(constants::MACH_WILDCARD_MESSAGE));
            }
            Ok(Self(MachServiceName::new(&raw).ok()))
        }
    }

    Ok(Vec::<Name>::deserialize(deserializer)?
        .into_iter()
        .filter_map(|name| name.0)
        .collect())
}

struct AllowedEntry(Option<DomainPattern>);

impl<'de> Deserialize<'de> for AllowedEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        if !domain_rules::is_valid_allowed_entry(&raw) {
            return Err(de::Error::custom(trogon_sandbox_protocol::ALLOWED_DOMAIN_ENTRY_MESSAGE));
        }
        Ok(Self(domain_rules::allowed_pattern(&raw)))
    }
}

struct DeniedEntry(Option<DeniedDomain>);

impl<'de> Deserialize<'de> for DeniedEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        if !domain_rules::is_valid_denied_entry(&raw) {
            return Err(de::Error::custom(trogon_sandbox_protocol::DENIED_DOMAIN_ENTRY_MESSAGE));
        }
        Ok(Self(domain_rules::denied_pattern(&raw)))
    }
}

fn allowed_entries<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<DomainPattern>, D::Error> {
    let entries = Vec::<AllowedEntry>::deserialize(deserializer)?;
    Ok(entries.into_iter().filter_map(|entry| entry.0).collect())
}

fn denied_entries<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<DeniedDomain>, D::Error> {
    let entries = Vec::<DeniedEntry>::deserialize(deserializer)?;
    Ok(entries.into_iter().filter_map(|entry| entry.0).collect())
}

/// Why a settings document was not accepted, with the path of the option.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
#[error("{}", self.render())]
pub struct SettingsError {
    path: String,
    message: String,
    more: Vec<SettingsError>,
}

impl SettingsError {
    fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
            more: Vec::new(),
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// The options that failed after this one, in the order of the schema.
    pub fn others(&self) -> &[SettingsError] {
        &self.more
    }

    fn render(&self) -> String {
        let mut text = if self.path.is_empty() || self.path == "." {
            self.message.clone()
        } else {
            format!("{}: {}", dotted_indices(&self.path), self.message)
        };
        for other in &self.more {
            text.push_str("; ");
            text.push_str(&other.render());
        }
        text
    }

    fn from_serde(err: serde_path_to_error::Error<serde_json::Error>) -> Self {
        let path = err.path().to_string();
        let message = err.inner().to_string();
        let message = message.split(" at line ").next().unwrap_or(&message).to_string();
        let message = issue_wording::plain(&message);
        if let Some(field) = message
            .strip_prefix("missing field `")
            .and_then(|rest| rest.strip_suffix('`'))
        {
            let path = if path == "." {
                field.to_string()
            } else {
                format!("{path}.{field}")
            };
            return Self::new(path, "Required");
        }
        Self::new(path, message)
    }
}

fn dotted_indices(path: &str) -> String {
    let mut text = String::with_capacity(path.len());
    for ch in path.chars() {
        match ch {
            '[' => text.push('.'),
            ']' => {}
            _ => text.push(ch),
        }
    }
    text
}

#[derive(Debug)]
struct Address(IpRange);

fn parse_address_range(entry: &str) -> Option<IpNet> {
    let (address, prefix) = match entry.split_once('/') {
        Some((address, prefix)) => (address, Some(prefix)),
        None => (entry, None),
    };
    let address = address.split('%').next().unwrap_or_default();
    if !domain_rules::is_ip(address) {
        return None;
    }
    let ip = IpAddr::from_str(address).ok()?;
    let max: u8 = if ip.is_ipv6() { 128 } else { 32 };
    let prefix = match prefix {
        None => max,
        Some(raw) => {
            if raw.is_empty() || raw.len() > 3 || !raw.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let value: u8 = raw.parse().ok()?;
            if value > max {
                return None;
            }
            value
        }
    };
    if let IpAddr::V6(v6) = ip
        && let Some(mapped) = v6.to_ipv4_mapped()
    {
        if prefix < 96 {
            return None;
        }
        return IpNet::new(IpAddr::V4(mapped), prefix - 96).ok();
    }
    IpNet::new(ip, prefix).ok()
}

impl<'de> Deserialize<'de> for Address {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let net = parse_address_range(&text).ok_or_else(|| de::Error::custom(constants::ADDRESS_RANGE_MESSAGE))?;
        Ok(Self(IpRange::new(net.to_string()).map_err(de::Error::custom)?))
    }
}

#[derive(Debug)]
struct FsEntry {
    text: String,
    literal: bool,
}

impl FsEntry {
    fn new(text: &str, literal: bool) -> Result<Self, FsPathError> {
        if text.is_empty() {
            return Err(FsPathError::Empty);
        }
        Ok(Self {
            text: text.to_string(),
            literal,
        })
    }

    fn resolve(&self, host: Option<&HostPaths>, kind: ListKind) -> Result<ResolvedEntry, FsPathError> {
        if let Some(host) = host {
            return host.resolve(&self.text, self.literal, kind);
        }
        let mut resolved = ResolvedEntry::default();
        if self.literal {
            resolved.entries.push(fs_path::literal(&self.text)?);
            return Ok(resolved);
        }
        let stripped = remove_trailing_glob_suffix(&self.text);
        let entry = match fs_path::glob(&stripped) {
            Err(err) if err.is_not_a_glob() => fs_path::literal(&stripped)?,
            other => other?,
        };
        resolved.entries.push(entry);
        Ok(resolved)
    }
}

impl<'de> Deserialize<'de> for FsEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        enum PathField {
            Text(String),
            Other,
        }

        impl<'de> Deserialize<'de> for PathField {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                match issue_wording::Loose::deserialize(deserializer)? {
                    issue_wording::Loose::Text(text) if text.is_empty() => {
                        Err(de::Error::custom(constants::PATH_EMPTY))
                    }
                    issue_wording::Loose::Text(text) => Ok(Self::Text(text)),
                    _ => Ok(Self::Other),
                }
            }
        }

        struct EntryVisitor;

        impl<'de> Visitor<'de> for EntryVisitor {
            type Value = FsEntry;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(constants::FILESYSTEM_ENTRY)
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<FsEntry, E> {
                if text.is_empty() {
                    return Err(E::custom(constants::PATH_EMPTY));
                }
                FsEntry::new(text, false).map_err(E::custom)
            }

            fn visit_bool<E: de::Error>(self, _: bool) -> Result<FsEntry, E> {
                Err(E::custom(issue_wording::aborting(constants::FILESYSTEM_ENTRY)))
            }

            fn visit_i64<E: de::Error>(self, _: i64) -> Result<FsEntry, E> {
                Err(E::custom(issue_wording::aborting(constants::FILESYSTEM_ENTRY)))
            }

            fn visit_u64<E: de::Error>(self, _: u64) -> Result<FsEntry, E> {
                Err(E::custom(issue_wording::aborting(constants::FILESYSTEM_ENTRY)))
            }

            fn visit_f64<E: de::Error>(self, _: f64) -> Result<FsEntry, E> {
                Err(E::custom(issue_wording::aborting(constants::FILESYSTEM_ENTRY)))
            }

            fn visit_unit<E: de::Error>(self) -> Result<FsEntry, E> {
                Err(E::custom(issue_wording::aborting(constants::FILESYSTEM_ENTRY)))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, _: A) -> Result<FsEntry, A::Error> {
                Err(de::Error::custom(issue_wording::aborting(constants::FILESYSTEM_ENTRY)))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<FsEntry, A::Error> {
                let mut path: Option<PathField> = None;
                let mut literal: Option<serde_json::Value> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "path" => path = Some(map.next_value()?),
                        "literal" => literal = Some(map.next_value()?),
                        other => {
                            return Err(de::Error::unknown_field(other, &["path", "literal"]));
                        }
                    }
                }
                match (path, literal) {
                    (Some(PathField::Text(path)), Some(serde_json::Value::Bool(true))) => {
                        FsEntry::new(&path, true).map_err(de::Error::custom)
                    }
                    _ => Err(de::Error::custom(issue_wording::aborting(constants::FILESYSTEM_ENTRY))),
                }
            }
        }

        deserializer.deserialize_any(EntryVisitor)
    }
}

#[derive(Debug)]
struct AbsoluteBinary(PathBuf);

impl<'de> Deserialize<'de> for AbsoluteBinary {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text.is_empty() {
            return Err(de::Error::custom(issue_wording::encode(&[
                ("", constants::PATH_EMPTY),
                ("", constants::BINARY_NOT_ABSOLUTE),
            ])));
        }
        let path = PathBuf::from(text);
        if !path.is_absolute() {
            return Err(de::Error::custom(constants::BINARY_NOT_ABSOLUTE));
        }
        Ok(Self(path))
    }
}

#[derive(Debug)]
struct NonEmptyPath(PathBuf);

impl<'de> Deserialize<'de> for NonEmptyPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text.is_empty() {
            return Err(de::Error::custom(constants::NON_EMPTY));
        }
        Ok(Self(PathBuf::from(text)))
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct ProxyPort(Port);

impl<'de> Deserialize<'de> for ProxyPort {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let port = issue_wording::integer(deserializer, Some(1), Some(65535))?;
        let port = u16::try_from(port).map_err(de::Error::custom)?;
        Port::new(port).map(Self).map_err(de::Error::custom)
    }
}

fn tunnel_cap<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<TunnelCap>, D::Error> {
    let cap = issue_wording::integer(deserializer, Some(1), Some(65536))?;
    let cap = u32::try_from(cap).map_err(de::Error::custom)?;
    TunnelCap::new(cap).map(Some).map_err(de::Error::custom)
}

struct ReasonText(DenyReason);

impl<'de> Deserialize<'de> for ReasonText {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text.is_empty() {
            return Err(de::Error::custom(constants::NON_EMPTY));
        }
        DenyReason::new(&text).map(Self).map_err(de::Error::custom)
    }
}

fn denied_reasons<'de, D: Deserializer<'de>>(deserializer: D) -> Result<BTreeMap<String, DenyReason>, D::Error> {
    let reasons = BTreeMap::<String, ReasonText>::deserialize(deserializer)?;
    Ok(reasons.into_iter().map(|(domain, reason)| (domain, reason.0)).collect())
}

struct PlainDomain(Option<DomainPattern>);

impl<'de> Deserialize<'de> for PlainDomain {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        if !domain_rules::is_valid_domain_pattern(&raw) {
            return Err(de::Error::custom(constants::DOMAIN_PATTERN));
        }
        Ok(Self(domain_rules::plain_pattern(&raw)))
    }
}

fn plain_domains<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<DomainPattern>, D::Error> {
    let entries = Vec::<PlainDomain>::deserialize(deserializer)?;
    Ok(entries.into_iter().filter_map(|entry| entry.0).collect())
}

fn mitm_domains<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<DomainPattern>, D::Error> {
    let entries = Vec::<PlainDomain>::deserialize(deserializer)?;
    if entries.is_empty() {
        return Err(de::Error::custom(constants::ARRAY_NON_EMPTY));
    }
    Ok(entries.into_iter().filter_map(|entry| entry.0).collect())
}

#[derive(Debug)]
struct ProxyUrl(String);

impl<'de> Deserialize<'de> for ProxyUrl {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if url::Url::parse(&text).is_err() {
            return Err(de::Error::custom(constants::INVALID_URL));
        }
        ParentProxyUrl::new(&text).map_err(de::Error::custom)?;
        Ok(Self(text))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MitmProxySettings {
    socket_path: NonEmptyPath,
    #[serde(deserialize_with = "mitm_domains")]
    domains: Vec<DomainPattern>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ParentProxySettings {
    #[serde(default, deserialize_with = "present")]
    http: Option<ProxyUrl>,
    #[serde(default, deserialize_with = "present")]
    https: Option<ProxyUrl>,
    #[serde(default, deserialize_with = "present")]
    no_proxy: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TlsTerminateSettings {
    #[serde(default, deserialize_with = "tunnel_cap")]
    max_tunnels: Option<TunnelCap>,
    #[serde(default, deserialize_with = "present")]
    ca_cert_path: Option<NonEmptyPath>,
    #[serde(default, deserialize_with = "present")]
    ca_key_path: Option<NonEmptyPath>,
    #[serde(default, deserialize_with = "plain_domains")]
    exclude_domains: Vec<DomainPattern>,
    #[serde(default)]
    extra_ca_cert_paths: Vec<NonEmptyPath>,
    #[serde(default, deserialize_with = "present")]
    handshake_timeout_ms: Option<HandshakeTimeoutMs>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GitSettings {
    #[serde(default)]
    safe_directories: Vec<LiteralPath>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NetworkSettings {
    #[serde(deserialize_with = "allowed_entries")]
    allowed_domains: Vec<DomainPattern>,
    #[serde(deserialize_with = "denied_entries")]
    denied_domains: Vec<DeniedDomain>,
    #[serde(default)]
    denied_resolved_addresses: Vec<Address>,
    #[serde(default)]
    refuse_opaque_tunnels: bool,
    #[serde(default, deserialize_with = "denied_reasons")]
    denied_domain_reasons: BTreeMap<String, DenyReason>,
    #[serde(default)]
    strict_allowlist: bool,
    #[serde(default)]
    allow_unix_sockets: Vec<String>,
    #[serde(default)]
    allow_all_unix_sockets: bool,
    #[serde(default)]
    allow_local_binding: bool,
    #[serde(default, deserialize_with = "mach_services")]
    allow_mach_lookup: Vec<MachServiceName>,
    #[serde(default, deserialize_with = "present")]
    http_proxy_port: Option<ProxyPort>,
    #[serde(default, deserialize_with = "present")]
    socks_proxy_port: Option<ProxyPort>,
    #[serde(default, deserialize_with = "object_present")]
    mitm_proxy: Option<MitmProxySettings>,
    #[serde(default, deserialize_with = "object_present")]
    parent_proxy: Option<ParentProxySettings>,
    #[serde(default, deserialize_with = "object_present")]
    tls_terminate: Option<TlsTerminateSettings>,
    #[serde(default, deserialize_with = "present")]
    allow_plaintext_header_set: Option<bool>,
    #[serde(default)]
    strip_response_headers: Vec<StrippedHeader>,
    #[serde(default, deserialize_with = "present")]
    require_host_match: Option<HostMatchSetting>,
    #[serde(default, deserialize_with = "present")]
    ambiguous_request_target: Option<AmbiguousTarget>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FilesystemSettings {
    deny_read: Vec<FsEntry>,
    allow_write: Vec<FsEntry>,
    deny_write: Vec<FsEntry>,
    #[serde(default)]
    allow_read: Vec<FsEntry>,
    #[serde(default)]
    disabled: bool,
    #[serde(default)]
    allow_git_config: bool,
}

/// A settings document that passed validation.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SandboxSettings {
    #[serde(deserialize_with = "object")]
    network: NetworkSettings,
    #[serde(deserialize_with = "object")]
    filesystem: FilesystemSettings,
    #[serde(default)]
    ignore_violations: BTreeMap<String, Vec<String>>,
    #[serde(default, deserialize_with = "present")]
    bwrap_path: Option<AbsoluteBinary>,
    #[serde(default, deserialize_with = "object_present")]
    credentials: Option<CredentialSettings>,
    #[serde(default)]
    enable_weaker_nested_sandbox: bool,
    #[serde(default)]
    allow_nested_user_namespaces: bool,
    #[serde(default)]
    enable_weaker_network_isolation: bool,
    #[serde(default)]
    allow_apple_events: bool,
    #[serde(default)]
    allow_pty: bool,
    #[serde(default)]
    drop_linux_write_globs: bool,
    #[serde(default, deserialize_with = "object_present")]
    ripgrep: Option<RipgrepSettings>,
    #[serde(default, deserialize_with = "present")]
    mandatory_deny_search_depth: Option<SearchDepth>,
    #[serde(default, deserialize_with = "object_present")]
    seccomp: Option<SeccompSettings>,
    #[serde(default, deserialize_with = "present")]
    socat_path: Option<AbsoluteBinary>,
    #[serde(default, deserialize_with = "present")]
    java_agent_jar_path: Option<AbsoluteBinary>,
    #[serde(default, deserialize_with = "object_present")]
    windows: Option<windows_section::WindowsSection>,
    #[serde(default, deserialize_with = "object_present")]
    git: Option<GitSettings>,
    #[serde(skip)]
    resolved: Option<ResolvedFilesystem>,
    #[serde(skip)]
    unix_sockets: Vec<LiteralPath>,
    #[serde(skip)]
    ignored_keys: Vec<String>,
    #[serde(skip)]
    warnings: Vec<String>,
    #[serde(skip)]
    host: Option<HostPaths>,
}

#[derive(Debug)]
struct ResolvedFilesystem {
    policy: FsPolicy,
    credential_deny: Vec<FsPathEntry>,
    host_view: bool,
}

impl SandboxSettings {
    /// The settings in `json`, or the first option that is wrong.
    pub fn from_json(json: &str) -> Result<Self, SettingsError> {
        let mut deserializer = serde_json::Deserializer::from_str(json);
        let settings: Self = serde_path_to_error::deserialize(&mut deserializer)
            .map_err(|err| issues::report(json, false).unwrap_or_else(|| SettingsError::from_serde(err)))?;
        deserializer
            .end()
            .map_err(|err| SettingsError::new("", err.to_string()))?;
        settings.refined(json)?.resolve_filesystem(None)
    }

    /// Like [`Self::from_json`], for a runtime that shows the host
    /// filesystem: `~` and relative entries are resolved against `host`, and
    /// an empty filesystem section means everything readable and nothing
    /// writable.
    pub fn from_json_for_host(json: &str, host: &HostPaths) -> Result<Self, SettingsError> {
        let mut deserializer = serde_json::Deserializer::from_str(json);
        let settings: Self = serde_path_to_error::deserialize(&mut deserializer)
            .map_err(|err| issues::report(json, false).unwrap_or_else(|| SettingsError::from_serde(err)))?;
        deserializer
            .end()
            .map_err(|err| SettingsError::new("", err.to_string()))?;
        settings.refined(json)?.resolve_filesystem(Some(host))
    }

    /// Like [`Self::from_json`], reading the document leniently: a
    /// key it does not define is dropped instead of refused, wherever the
    /// schema ignores extra keys. `host` is as in [`Self::from_json_for_host`].
    /// What was dropped is listed in [`Self::warnings`].
    pub fn from_json_lenient(json: &str, host: Option<&HostPaths>) -> Result<Self, SettingsError> {
        let mut document: serde_json::Value =
            serde_json::from_str(json).map_err(|err| SettingsError::new("", err.to_string()))?;
        let mut dropped = Vec::new();
        let mut settings: Self = loop {
            match serde_path_to_error::deserialize(&document) {
                Ok(settings) => break settings,
                Err(err) => match dropped_key(&err) {
                    Some((container, key)) if remove_key(&mut document, &container, &key) => {
                        dropped.push(if container.is_empty() {
                            key
                        } else {
                            format!("{container}.{key}")
                        });
                    }
                    _ => {
                        return Err(
                            issues::report_value(document, true).unwrap_or_else(|| SettingsError::from_serde(err))
                        );
                    }
                },
            }
        };
        settings.ignored_keys = dropped;
        if let Some(err) = refine::chain(refine::issues(&document)) {
            return Err(err);
        }
        settings.resolve_filesystem(host)
    }

    fn refined(self, json: &str) -> Result<Self, SettingsError> {
        let document: serde_json::Value =
            serde_json::from_str(json).map_err(|err| SettingsError::new("", err.to_string()))?;
        match refine::chain(refine::issues(&document)) {
            Some(err) => Err(err),
            None => Ok(self),
        }
    }

    fn resolve_filesystem(mut self, host: Option<&HostPaths>) -> Result<Self, SettingsError> {
        self.validate_network()?;
        self.host = host.cloned();
        let filesystem = &self.filesystem;
        let mut warnings: Vec<String> = self
            .ignored_keys
            .iter()
            .map(|key| format!("Ignoring unknown setting: {key}"))
            .collect();
        if cfg!(target_os = "linux") {
            if self.ripgrep.is_some() {
                warnings.push("ripgrep is not used: this runtime scans for mandatory deny paths itself".to_string());
            }
            if self.seccomp.is_some() {
                warnings.push(
                    "seccomp.applyPath and seccomp.argv0 are not used: this runtime builds its own seccomp filter"
                        .to_string(),
                );
            }
        }
        let drop_globs = self.drop_linux_write_globs && cfg!(target_os = "linux");
        let enforced = !filesystem.disabled;
        let deny_read = resolve_list(
            &filesystem.deny_read,
            "denyRead",
            ListKind::Deny,
            host,
            false,
            enforced,
            &mut warnings,
        )?;
        let credential_deny = match &self.credentials {
            Some(credentials) => credentials
                .deny_entries(host, &mut warnings)?
                .into_iter()
                .filter(|entry| {
                    enforced || !matches!(entry, FsPathEntry::Glob { pattern } if pattern.ends_with_separator())
                })
                .collect(),
            None => Vec::new(),
        };
        let allow_read = resolve_list(
            &filesystem.allow_read,
            "allowRead",
            ListKind::Allow,
            host,
            false,
            enforced,
            &mut warnings,
        )?;
        let allow_write = resolve_list(
            &filesystem.allow_write,
            "allowWrite",
            ListKind::Allow,
            host,
            drop_globs,
            enforced,
            &mut warnings,
        )?;
        let deny_write = resolve_list(
            &filesystem.deny_write,
            "denyWrite",
            ListKind::Deny,
            host,
            drop_globs,
            enforced,
            &mut warnings,
        )?;
        let policy = FsPolicy::new(
            deny_read,
            credential_deny.clone(),
            allow_read,
            Vec::new(),
            allow_write,
            deny_write,
        )
        .map_err(|err| SettingsError::new("filesystem", err.to_string()))?;
        if cfg!(target_os = "linux") {
            warnings.extend(rootless_read_globs(&policy));
        }
        let mut unix_sockets = Vec::new();
        for (index, text) in self.network.allow_unix_sockets.iter().enumerate() {
            if text.is_empty() {
                continue;
            }
            let path = format!("network.allowUnixSockets[{index}]");
            let entry = FsEntry::new(text, true).map_err(|err| SettingsError::new(&path, err.to_string()))?;
            let resolved = entry
                .resolve(host, ListKind::Allow)
                .map_err(|err| SettingsError::new(&path, err.to_string()))?;
            warnings.extend(resolved.warnings);
            for found in resolved.entries {
                if let FsPathEntry::Literal { path, .. } = found
                    && !unix_sockets.contains(&path)
                {
                    unix_sockets.push(path);
                }
            }
        }
        if cfg!(target_os = "linux") && !unix_sockets.is_empty() {
            warnings.push(
                "network.allowUnixSockets is ignored on Linux: seccomp cannot filter by path. Use allowAllUnixSockets"
                    .to_string(),
            );
        }
        if cfg!(target_os = "linux") && self.network.allow_all_unix_sockets {
            warnings.push("Skipping the unix socket seccomp rule - allowAllUnixSockets is enabled".to_string());
        }
        self.unix_sockets = unix_sockets;
        self.resolved = Some(ResolvedFilesystem {
            policy,
            credential_deny,
            host_view: host.is_some(),
        });
        self.warnings = warnings;
        Ok(self)
    }

    fn validate_network(&self) -> Result<(), SettingsError> {
        let network = &self.network;
        if network.tls_terminate.is_some() && network.mitm_proxy.is_some() {
            return Err(SettingsError::new(
                "",
                "network.tlsTerminate and network.mitmProxy are mutually exclusive",
            ));
        }
        if let Some(tls) = &network.tls_terminate
            && tls.ca_cert_path.is_some() != tls.ca_key_path.is_some()
        {
            return Err(SettingsError::new(
                "network.tlsTerminate",
                "caCertPath and caKeyPath must be provided together",
            ));
        }
        let external = network.http_proxy_port.is_some() && network.socks_proxy_port.is_some();
        if external
            && let Some(tls) = &network.tls_terminate
            && (tls.ca_cert_path.is_some() || !tls.exclude_domains.is_empty() || !tls.extra_ca_cert_paths.is_empty())
        {
            return Err(SettingsError::new(
                "network.tlsTerminate",
                "tlsTerminate options need the runtime's own proxy, which httpProxyPort and socksProxyPort together replace",
            ));
        }
        if external && self.credentials.is_some() {
            return Err(SettingsError::new(
                "credentials",
                "credentials need the runtime's own proxy, which httpProxyPort and socksProxyPort together replace",
            ));
        }
        Ok(())
    }

    /// The proxy behaviour these settings ask for. Environment variables are
    /// read for the parent proxy only when `network.parentProxy` is present.
    pub fn network_options(&self) -> Result<NetworkOptions, SettingsError> {
        self.network_options_with_env(&SystemEnv)
    }

    fn network_options_with_env(&self, env: &impl ReadEnv) -> Result<NetworkOptions, SettingsError> {
        let network = &self.network;
        let mut options = NetworkOptions::default();
        if !network.denied_domain_reasons.is_empty() {
            options.denied_domain_reasons = DeniedDomainReasons::from(network.denied_domain_reasons.clone());
        }
        if network.strict_allowlist {
            options.unlisted = UnlistedDestinations::Refuse;
        }
        if let Some(parent) = &network.parent_proxy {
            options.parent_proxy = resolve_parent_proxy(
                parent.http.as_ref().map(|url| url.0.as_str()),
                parent.https.as_ref().map(|url| url.0.as_str()),
                parent.no_proxy.as_deref(),
                env,
            );
        }
        if let Some(mitm) = &network.mitm_proxy {
            options.mitm = Some(MitmUnixSocket {
                socket_path: self.resolved_against_cwd(&mitm.socket_path.0),
                domains: mitm.domains.clone(),
            });
        }
        options.denied_resolved_addresses = network
            .denied_resolved_addresses
            .iter()
            .map(|address| address.0.clone())
            .collect();
        options.external_proxy = match (network.http_proxy_port, network.socks_proxy_port) {
            (Some(http), Some(socks)) => Some(ExternalProxy::both(http.0, socks.0)),
            (Some(port), None) => Some(ExternalProxy::http_only(port.0)),
            (None, Some(port)) => Some(ExternalProxy::socks_only(port.0)),
            (None, None) => None,
        };
        if let Some(tls) = &network.tls_terminate {
            options.tunnel_cap = tls.max_tunnels;
            options.terminate_allowed_tls = AllowedTlsTermination::Enabled;
            if let (Some(cert), Some(key)) = (&tls.ca_cert_path, &tls.ca_key_path) {
                options.ca_files = Some(CaFiles::new(cert.0.clone(), key.0.clone()));
            }
            if !tls.exclude_domains.is_empty() {
                options.tls_exclusions = TlsExclusions::new(tls.exclude_domains.iter().cloned());
            }
            if !tls.extra_ca_cert_paths.is_empty() {
                options.extra_ca_cert_paths = ExtraCaCertPaths::new(
                    tls.extra_ca_cert_paths
                        .iter()
                        .map(|path| self.resolved_against_cwd(&path.0)),
                );
            }
            options.handshake_timeout = tls.handshake_timeout_ms.as_ref().map(|timeout| timeout.0);
        }
        if let Some(mode) = &network.ambiguous_request_target {
            options.ambiguous_targets = mode.0;
        }
        if !network.strip_response_headers.is_empty() {
            options.strip_response_headers = ResponseHeaderStrip::new(network.strip_response_headers.iter().cloned());
        }
        if network.allow_plaintext_header_set == Some(true) {
            options.plaintext_header_set = PlaintextHeaderSet::Enabled;
        }
        Ok(options)
    }

    /// A relative path is read from the working directory, as when the file is
    /// opened.
    fn resolved_against_cwd(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            return path.to_path_buf();
        }
        let cwd = match &self.host {
            Some(host) => Some(host.cwd().to_path_buf()),
            None => std::env::current_dir().ok(),
        };
        cwd.map_or_else(|| path.to_path_buf(), |cwd| cwd.join(path))
    }

    /// The opt-ins these settings ask for on the platform this runs on. The
    /// macOS only keys (`allowPty`, `allowAppleEvents`, `allowUnixSockets`,
    /// `allowMachLookup`, `allowLocalBinding`) are ignored elsewhere and the
    /// Linux only keys (`enableWeakerNestedSandbox`,
    /// `allowNestedUserNamespaces`) are ignored off Linux, so
    /// one file serves both platforms.
    pub fn platform_options(&self) -> PlatformOptions {
        self.platform_options_for(cfg!(target_os = "macos"), cfg!(target_os = "linux"))
    }

    fn platform_options_for(&self, macos: bool, linux: bool) -> PlatformOptions {
        let mut options = PlatformOptions::default();
        if macos {
            options.macos.allow_pty = self.allow_pty.into();
            options.macos.allow_local_binding = self.network.allow_local_binding.into();
            options.macos.allow_mach_lookup = self.network.allow_mach_lookup.clone();
            options.macos.allow_trust_service = self.enable_weaker_network_isolation.into();
            options.macos.allow_apple_events = self.allow_apple_events.into();
            options.macos.allow_all_unix_sockets = self.network.allow_all_unix_sockets.into();
            if !self.network.allow_all_unix_sockets {
                options.macos.allow_unix_sockets = self.unix_sockets.clone();
            }
        }
        if linux {
            options.linux.weaker_nested_sandbox = self.enable_weaker_nested_sandbox.into();
            options.linux.allow_nested_user_namespaces = self.allow_nested_user_namespaces.into();
            options.linux.allow_all_unix_sockets = self.network.allow_all_unix_sockets.into();
            options.filesystem.mandatory_deny_search_depth = self
                .mandatory_deny_search_depth
                .as_ref()
                .and_then(|depth| MandatoryDenySearchDepth::new(depth.get()).ok());
        }
        options.filesystem.allow_git_config = self.filesystem.allow_git_config.into();
        options.filesystem.disabled = self.filesystem.disabled.into();
        if let Some(git) = &self.git {
            options.git_safe_directories = git.safe_directories.clone();
        }
        options
    }

    /// The paths the `credentials.files` entries in `deny` mode name.
    pub fn credential_deny_paths(&self) -> &[FsPathEntry] {
        self.resolved
            .as_ref()
            .map_or(&[], |resolved| resolved.credential_deny.as_slice())
    }

    /// What can change while commands run: the filesystem and network rules.
    pub fn policy(&self) -> Result<RuntimePolicy, SettingsError> {
        let resolved = self
            .resolved
            .as_ref()
            .ok_or_else(|| SettingsError::new("filesystem", "the filesystem section was not resolved"))?;
        let fs = resolved.policy.clone();
        let network = NetworkPolicy::UserDefined {
            rules: self
                .network
                .allowed_domains
                .iter()
                .map(|domain| DomainRule {
                    domain: domain.clone(),
                    action: DomainRuleAction::PassThrough,
                })
                .collect(),
            allow_ranges: self
                .network
                .allowed_domains
                .iter()
                .filter(|pattern| pattern.port().is_none())
                .filter_map(|pattern| {
                    let PatternTarget::Exact(host) = pattern.target() else {
                        return None;
                    };
                    let address = host.as_ip()?;
                    let prefix = if address.is_ipv4() { 32 } else { 128 };
                    IpRange::new(format!("{address}/{prefix}")).ok()
                })
                .collect(),
            deny_ranges: Vec::new(),
            denied_domains: self.network.denied_domains.clone(),
            opaque_tunnels: if self.network.refuse_opaque_tunnels {
                OpaqueTunnelPolicy::Refuse
            } else {
                OpaqueTunnelPolicy::Allow
            },
            request_rules: Vec::new(),
        };
        Ok(RuntimePolicy {
            fs: Some(fs).filter(|fs| resolved.host_view || !fs.is_empty()),
            network,
        })
    }

    /// What the operator should hear about how entries were read. Meant for
    /// debug logs; none of it changes what is enforced.
    /// Whether `network.allowLocalBinding` is set.
    pub fn allow_local_binding(&self) -> bool {
        self.network.allow_local_binding
    }

    /// The explicit `network.requireHostMatch`, when the settings give one. The
    /// network options cannot carry it, so the caller applies it to the proxy's
    /// request guard and picks its own default when this is `None`.
    pub fn host_match(&self) -> Option<HostMatch> {
        self.network.require_host_match.as_ref().map(|host_match| host_match.0)
    }

    /// Whether `network.tlsTerminate` is present, which is what makes the
    /// runtime build a CA at all.
    pub fn tls_terminates(&self) -> bool {
        self.network.tls_terminate.is_some()
    }

    /// The `windows` section: how the fence and `trogon-sandbox-fence-helper` are found.
    pub fn windows(&self) -> Result<WindowsSettings, SettingsError> {
        WindowsSettings::read(self.windows.as_ref())
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn ignore_violations(&self) -> IgnoreViolations {
        IgnoreViolations::new(self.ignore_violations.clone())
    }

    pub fn allowed_domains(&self) -> Vec<String> {
        self.network.allowed_domains.iter().map(ToString::to_string).collect()
    }

    pub fn bwrap_path(&self) -> Option<&Path> {
        self.bwrap_path.as_ref().map(|binary| binary.0.as_path())
    }

    pub fn socat_path(&self) -> Option<&Path> {
        self.socat_path.as_ref().map(|binary| binary.0.as_path())
    }

    /// The jar `javaAgentJarPath` names, for
    /// [`PolicyExecBuilder::tool_proxy_env`](crate::PolicyExecBuilder::tool_proxy_env).
    pub fn java_agent_jar_path(&self) -> Option<&Path> {
        self.java_agent_jar_path.as_ref().map(|binary| binary.0.as_path())
    }

    /// A config that runs commands in `workspace` under these settings.
    pub fn into_config(self, workspace: impl Into<PathBuf>) -> Result<PolicyExecConfig, SettingsError> {
        self.to_config(workspace)
    }

    /// The standard config for these settings: [`Self::to_config`] with names
    /// allowed to resolve into private-use ranges, `localhost` and
    /// `*.localhost` allowed to resolve to loopback, names decided by name
    /// alone and refusals answered with their reason, none of which a settings
    /// key reaches, and the Linux scan for protected names three levels deep
    /// when the settings leave that unset. The filesystem is the host's when
    /// the settings were loaded for a host. Whether the Host header must match
    /// the TLS server name is not a network option; read it with
    /// [`Self::host_match`]. Pair the config with
    /// [`PolicyExecBuilder::tool_proxy_env_from`](crate::PolicyExecBuilder::tool_proxy_env_from)
    /// for the tool proxy variables, or use
    /// [`PolicyExec::standard_builder`](crate::PolicyExec::standard_builder).
    pub fn to_standard_config(&self, workspace: impl Into<PathBuf>) -> Result<PolicyExecConfig, SettingsError> {
        let mut options = self.network_options()?;
        options.private_ranges_by_name = PrivateRangesByName::Enabled;
        options.loopback_names = LoopbackNames::Enabled;
        options.names_matched_by_name = NamesMatchedByName::Enabled;
        options.reasoned_refusals = ReasonedRefusals::Enabled;
        let mut config = self.to_config_with_network_options(workspace, options)?;
        let mut platform = config.configured_platform_options().clone();
        if cfg!(target_os = "linux") && self.mandatory_deny_search_depth.is_none() {
            platform.filesystem.mandatory_deny_search_depth = Some(MandatoryDenySearchDepth::DEFAULT);
        }
        if cfg!(target_os = "macos") {
            platform.macos.protected_names_anchor = ProtectedNamesAnchor::WorkingDirectory;
        }
        config = config.platform_options(platform);
        Ok(config)
    }

    /// What a command's own credentials or allowed domains are resolved
    /// against.
    fn credential_context(&self) -> CredentialContext {
        CredentialContext {
            session: self.credentials.clone(),
            session_deny: self
                .resolved
                .as_ref()
                .map(|resolved| resolved.credential_deny.clone())
                .unwrap_or_default(),
            allowed: self.network.allowed_domains.clone(),
            tls_terminate: self.network.tls_terminate.is_some(),
            excluded: self
                .network
                .tls_terminate
                .as_ref()
                .map_or(Vec::new(), |tls| tls.exclude_domains.clone()),
            host: self.host.clone(),
        }
    }

    /// Like [`Self::into_config`], without consuming the settings.
    pub fn to_config(&self, workspace: impl Into<PathBuf>) -> Result<PolicyExecConfig, SettingsError> {
        self.to_config_with_network_options(workspace, self.network_options()?)
    }

    /// Like [`Self::to_config`], with `options` standing in for the network
    /// options the settings describe, so a caller can layer extras on them.
    pub fn to_config_with_network_options(
        &self,
        workspace: impl Into<PathBuf>,
        options: NetworkOptions,
    ) -> Result<PolicyExecConfig, SettingsError> {
        let policy = self.policy()?;
        let mut config = PolicyExecConfig::new(workspace)
            .network(policy.network)
            .ignore_violations(self.ignore_violations())
            .network_options(options)
            .platform_options(self.platform_options());
        if let Some(mut fs) = policy.fs {
            if let Some(jar) = self.java_agent_jar_path().filter(|jar| jar.is_file())
                && let Ok(entry) = fs_path::literal(&jar.to_string_lossy())
                && !fs.allow_read_within_deny().contains(&entry)
            {
                let mut allow_read = fs.allow_read_within_deny().to_vec();
                allow_read.push(entry);
                fs = FsPolicy::new(
                    fs.deny_read().to_vec(),
                    fs.credential_deny_read().to_vec(),
                    allow_read,
                    fs.own_allow_within_deny().to_vec(),
                    fs.allow_write().to_vec(),
                    fs.deny_write().to_vec(),
                )
                .map_err(|err| SettingsError::new("filesystem", err.to_string()))?;
            }
            config = config.fs_policy(fs);
        }
        if let Some(bwrap) = self.bwrap_path() {
            config = config.bwrap_binary(bwrap);
        }
        if let Some(socat) = self.socat_path() {
            config = config.socat_binary(socat);
        }
        config = config.credential_context(self.credential_context());
        if let Some(credentials) = &self.credentials {
            let rules = credentials.rules(
                &self.network.allowed_domains,
                self.network.tls_terminate.is_some(),
                self.network
                    .tls_terminate
                    .as_ref()
                    .map_or(&[][..], |tls| tls.exclude_domains.as_slice()),
                self.host.as_ref(),
            )?;
            config = config
                .secret_bindings(rules.bindings)
                .aws_credential_pairs(rules.aws_pairs)
                .sigv4_policies(rules.sigv4)
                .plaintext_inject(rules.plaintext_inject);
        }
        if self.resolved.as_ref().is_some_and(|resolved| resolved.host_view) {
            config = config.view(FilesystemView::Host);
        }
        Ok(config)
    }
}

/// Resolves explicit settings first and the environment second (`HTTP_PROXY`,
/// `HTTPS_PROXY`, `NO_PROXY`, either case). `https` falls back to `http`. A URL
/// that does not parse is ignored; with none left there is no parent proxy.
fn resolve_parent_proxy(
    http: Option<&str>,
    https: Option<&str>,
    no_proxy: Option<&str>,
    env: &impl ReadEnv,
) -> Option<ParentProxy> {
    let lookup = |upper: &str, lower: &str| env.var(upper).ok().or_else(|| env.var(lower).ok());
    let http = http.map(str::to_string).or_else(|| lookup("HTTP_PROXY", "http_proxy"));
    let https = https
        .map(str::to_string)
        .or_else(|| lookup("HTTPS_PROXY", "https_proxy"))
        .or_else(|| http.clone());
    let no_proxy = no_proxy
        .map(str::to_string)
        .or_else(|| lookup("NO_PROXY", "no_proxy"))
        .unwrap_or_default();
    let parse = |raw: Option<String>| {
        raw.filter(|raw| !raw.is_empty())
            .and_then(|raw| ParentProxyUrl::new(&raw).ok())
    };
    ParentProxy::new(parse(http), parse(https), NoProxy::new(&no_proxy)).ok()
}

fn dropped_key(err: &serde_path_to_error::Error<serde_json::Error>) -> Option<(String, String)> {
    const LENIENT: [&str; 12] = [
        "",
        "credentials.files[]",
        "credentials.envVars[]",
        "network",
        "network.mitmProxy",
        "network.parentProxy",
        "network.tlsTerminate",
        "filesystem",
        "ripgrep",
        "seccomp",
        "git",
        "windows",
    ];
    let message = err.inner().to_string();
    let key = message.strip_prefix("unknown field `")?.split('`').next()?.to_string();
    let path = err.path().to_string();
    let rest = path.strip_suffix(&key)?;
    let container = rest.strip_suffix('.').unwrap_or(rest).to_string();
    LENIENT
        .contains(&without_indices(&container).as_str())
        .then_some((container, key))
}

fn without_indices(path: &str) -> String {
    let mut plain = String::with_capacity(path.len());
    let mut in_index = false;
    for ch in path.chars() {
        match ch {
            '[' => {
                in_index = true;
                plain.push_str("[]");
            }
            ']' => in_index = false,
            _ if !in_index => plain.push(ch),
            _ => {}
        }
    }
    plain
}

fn remove_key(document: &mut serde_json::Value, container: &str, key: &str) -> bool {
    let mut node = document;
    if !container.is_empty() {
        for segment in container.split('.') {
            let (name, indices) = segment
                .split_once('[')
                .map_or((segment, ""), |(name, rest)| (name, rest));
            match node.get_mut(name) {
                Some(next) => node = next,
                None => return false,
            }
            for index in indices.split('[').filter(|index| !index.is_empty()) {
                let Some(index) = index.trim_end_matches(']').parse::<usize>().ok() else {
                    return false;
                };
                match node.get_mut(index) {
                    Some(next) => node = next,
                    None => return false,
                }
            }
        }
    }
    node.as_object_mut().is_some_and(|object| object.remove(key).is_some())
}

fn rootless_read_globs(policy: &FsPolicy) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for entry in policy
        .deny_read()
        .iter()
        .chain(policy.credential_deny_read())
        .chain(policy.allow_read_within_deny())
    {
        let FsPathEntry::Glob { pattern } = entry else {
            continue;
        };
        let stripped = remove_trailing_glob_suffix(pattern.as_str());
        let starts_at_root = GlobPattern::new(&stripped).is_ok()
            && stripped
                .strip_prefix('/')
                .and_then(|rest| rest.split('/').next())
                .is_some_and(has_glob_chars);
        if starts_at_root && !found.iter().any(|text| text == pattern.as_str()) {
            found.push(pattern.as_str().to_string());
        }
    }
    found
        .into_iter()
        .map(|text| format!("Skipping glob read pattern on Linux, it has no directory to start from: {text}"))
        .collect()
}

fn resolve_list(
    list: &[FsEntry],
    name: &str,
    kind: ListKind,
    host: Option<&HostPaths>,
    drop_globs: bool,
    enforced: bool,
    warnings: &mut Vec<String>,
) -> Result<Vec<FsPathEntry>, SettingsError> {
    let mut entries = Vec::new();
    for (index, entry) in list.iter().enumerate() {
        let mut resolved = entry
            .resolve(host, kind)
            .map_err(|err| SettingsError::new(format!("filesystem.{name}[{index}]"), err.to_string()))?;
        warnings.append(&mut resolved.warnings);
        if kind == ListKind::Deny {
            let inert = resolved.entries.iter().find_map(|found| match found {
                FsPathEntry::Glob { pattern } if pattern.ends_with_separator() => Some(pattern),
                _ => None,
            });
            if let Some(pattern) = inert {
                if enforced {
                    return Err(SettingsError::new(
                        format!("filesystem.{name}[{index}]"),
                        inert_deny_glob_message(&entry.text, pattern.as_str()),
                    ));
                }
                continue;
            }
        }
        if drop_globs {
            let before = resolved.entries.len();
            resolved
                .entries
                .retain(|found| matches!(found, FsPathEntry::Literal { .. }));
            if resolved.entries.is_empty() && before > 0 {
                warnings.push(format!(
                    "Skipping glob write pattern on Linux: {}",
                    remove_trailing_glob_suffix(&entry.text)
                ));
            }
        }
        for found in resolved.entries {
            if !entries.contains(&found) {
                entries.push(found);
            }
        }
    }
    Ok(entries)
}

fn inert_deny_glob_message(spelled: &str, pattern: &str) -> String {
    format!(
        "Deny glob \"{spelled}\" ends in a separator, so as a pattern it can match no path. Write \"{}\", which is also read as the path of that name where it exists, or add a \"**\" segment to match at any depth.",
        pattern.trim_end_matches('/'),
    )
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum UnknownKeys {
    Reject,
    Strip,
}

/// What reading a settings file turned up.
#[derive(Debug)]
pub enum SettingsFile {
    Missing,
    Empty,
    Unreadable(std::io::Error),
    Invalid(SettingsError),
    NotJson(String),
    Loaded(Box<SandboxSettings>),
}

impl SettingsFile {
    pub fn read(path: &Path) -> Self {
        Self::read_with(path, None, UnknownKeys::Reject)
    }

    /// Like [`Self::read`], reading the file leniently, which drops
    /// a key it does not define. See [`SandboxSettings::from_json_lenient`].
    pub fn read_lenient(path: &Path, host: Option<&HostPaths>) -> Self {
        Self::read_with(path, host, UnknownKeys::Strip)
    }

    /// Like [`Self::read`], resolving the filesystem entries for the host
    /// view.
    pub fn read_for_host(path: &Path, host: &HostPaths) -> Self {
        Self::read_with(path, Some(host), UnknownKeys::Reject)
    }

    fn read_with(path: &Path, host: Option<&HostPaths>, unknown: UnknownKeys) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Self::Missing,
            Err(_) if cfg!(windows) && path.is_dir() => {
                return Self::Unreadable(std::io::ErrorKind::IsADirectory.into());
            }
            Err(err) => return Self::Unreadable(err),
        };
        if text.trim().is_empty() {
            return Self::Empty;
        }
        if let Err(err) = serde_json::from_str::<serde::de::IgnoredAny>(&text) {
            return Self::NotJson(err.to_string());
        }
        let parsed = match (unknown, host) {
            (UnknownKeys::Strip, _) => SandboxSettings::from_json_lenient(&text, host),
            (UnknownKeys::Reject, Some(host)) => SandboxSettings::from_json_for_host(&text, host),
            (UnknownKeys::Reject, None) => SandboxSettings::from_json(&text),
        };
        match parsed {
            Ok(settings) => Self::Loaded(Box::new(settings)),
            Err(err) => Self::Invalid(err),
        }
    }

    /// Why this file cannot be used, worded for a person, or `None` when it
    /// is loaded. A missing file is reported too; callers decide whether
    /// that one is fatal.
    pub fn problem(&self, path: &Path) -> Option<String> {
        let shown = path.display();
        match self {
            Self::Loaded(_) => None,
            Self::Missing => Some(format!("{shown} does not exist.")),
            Self::Empty => Some(format!("{shown} is empty. Delete it, or put a config in it.")),
            Self::Unreadable(err) => {
                let code = match (err.raw_os_error(), err.kind()) {
                    (Some(raw), _) => os_error_name(raw),
                    (None, std::io::ErrorKind::IsADirectory) => "EISDIR".to_owned(),
                    (None, kind) => format!("{kind:?}"),
                };
                Some(format!("{shown} could not be read ({code})."))
            }
            Self::NotJson(message) => Some(format!("{shown} is not valid JSON: {message}")),
            Self::Invalid(err) => Some(format!("{shown} does not hold a valid config - {err}")),
        }
    }
}

#[cfg(unix)]
fn os_error_name(raw: i32) -> String {
    format!("{:?}", nix::errno::Errno::from_raw(raw))
}

/// The POSIX style name of a Windows error code, the way a path error is
/// named on every other platform.
#[cfg(windows)]
fn os_error_name(raw: i32) -> String {
    match raw {
        2 | 3 | 123 | 161 => "ENOENT",
        5 => "EPERM",
        32 | 33 => "EBUSY",
        206 => "ENAMETOOLONG",
        1920 => "EACCES",
        1921 => "ELOOP",
        _ => return format!("os error {raw}"),
    }
    .to_owned()
}

#[cfg(test)]
mod tests;
