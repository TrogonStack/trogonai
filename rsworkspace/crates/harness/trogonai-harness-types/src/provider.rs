//! Which backend a turn's model calls are routed through.

use serde::de::{self, Deserializer};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::config::CredentialSource;
use crate::id::ModelId;

/// Maximum length of a configured provider identifier.
pub const MAX_CONFIGURED_PROVIDER_ID_BYTES: usize = 64;

/// Inline configured-provider identity carried by [`ProviderId::Configured`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredProviderIdentity {
    bytes: [u8; MAX_CONFIGURED_PROVIDER_ID_BYTES],
    len: u8,
    binding: Option<[u8; 32]>,
}

impl ConfiguredProviderIdentity {
    fn label(&self) -> &str {
        std::str::from_utf8(&self.bytes[..usize::from(self.len)]).expect("validated id is utf-8")
    }

    /// Whether this identity carries a verified registry binding.
    #[must_use]
    pub fn is_bound(&self) -> bool {
        self.binding.is_some()
    }
}

/// Registry surface [`ProviderId::bind`] needs from a configured-provider table.
pub trait ConfiguredProviderRegistry {
    /// Returns the stable binding identity for `id`, when that provider exists.
    fn binding_identity(&self, id: &str) -> Option<[u8; 32]>;
}

/// Why a configured provider id string was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ValidateConfiguredIdError {
    /// The id exceeded [`MAX_CONFIGURED_PROVIDER_ID_BYTES`].
    #[error("configured provider id exceeds the maximum length")]
    LimitExceeded,
    /// The id failed the configured-provider alphabet or shape rules.
    #[error("configured provider id is invalid")]
    InvalidProviderId,
    /// The id collides with a built-in route name.
    #[error("configured provider id is reserved")]
    ReservedProviderId,
}

/// Why [`ProviderId::bind`] refused to attach a binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProviderBindError {
    /// No configured provider with this name exists in the registry.
    #[error("unknown configured provider")]
    UnknownConfiguredProvider,
    /// The registry binding no longer matches what was saved.
    #[error("configured provider binding changed")]
    ConfiguredProviderChanged,
}

/// Why a persisted provider value could not be restored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid provider binding")]
pub struct InvalidProviderBinding;

/// Which backend serves a turn's model calls.
#[derive(Debug, Clone, Copy)]
pub enum ProviderId {
    /// The default route, through the AI Gateway.
    Gateway,
    /// Routed through a ChatGPT/Codex subscription.
    Codex,
    /// Routed through a Grok subscription.
    Grok,
    /// Routed through a configured provider declared in settings.
    Configured(ConfiguredProviderIdentity),
}

impl PartialEq for ProviderId {
    fn eq(&self, other: &Self) -> bool {
        self.eql(*other)
    }
}

impl Eq for ProviderId {}

impl ProviderId {
    /// Borrows the inline name; built-in names have static storage.
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::Gateway => "gateway",
            Self::Codex => "codex",
            Self::Grok => "grok",
            Self::Configured(identity) => identity.label(),
        }
    }

    /// Whether two providers name the same route, ignoring binding on configured ids.
    #[must_use]
    pub fn eql(self, other: Self) -> bool {
        match (self, other) {
            (Self::Gateway, Self::Gateway) | (Self::Codex, Self::Codex) | (Self::Grok, Self::Grok) => true,
            (Self::Configured(left), Self::Configured(right)) => left.label() == right.label(),
            _ => false,
        }
    }

    /// Attaches or verifies a configured provider's binding identity.
    pub fn bind(self, registry: &impl ConfiguredProviderRegistry) -> Result<Self, ProviderBindError> {
        let Self::Configured(mut identity) = self else {
            return Ok(self);
        };
        let definition = registry
            .binding_identity(identity.label())
            .ok_or(ProviderBindError::UnknownConfiguredProvider)?;
        if let Some(expected) = identity.binding
            && expected != definition
        {
            return Err(ProviderBindError::ConfiguredProviderChanged);
        }
        identity.binding = Some(definition);
        Ok(Self::Configured(identity))
    }

    /// Whether two providers name the same route with the same configured binding.
    #[must_use]
    pub fn same_authority(self, other: Self) -> bool {
        if !self.eql(other) {
            return false;
        }
        match (self, other) {
            (Self::Configured(left), Self::Configured(right)) => match (left.binding, right.binding) {
                (Some(first), Some(second)) => first == second,
                _ => false,
            },
            _ => true,
        }
    }

    /// Parses the runtime spelling of a provider, case-insensitively for built-ins.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        if value.eq_ignore_ascii_case("gateway") {
            return Some(Self::Gateway);
        }
        if value.eq_ignore_ascii_case("codex") {
            return Some(Self::Codex);
        }
        if value.eq_ignore_ascii_case("grok") {
            return Some(Self::Grok);
        }
        validate_configured_id(value).ok()?;
        let mut bytes = [0; MAX_CONFIGURED_PROVIDER_ID_BYTES];
        bytes[..value.len()].copy_from_slice(value.as_bytes());
        Some(Self::Configured(ConfiguredProviderIdentity {
            bytes,
            len: value.len().try_into().expect("validate_configured_id bounds len"),
            binding: None,
        }))
    }

    /// Whether a credential from `source` authorizes this provider.
    ///
    /// A [`CredentialSource::HostManaged`] source authorizes every provider, because the
    /// authority is the embedding host's rather than a route's. Otherwise a subscription source
    /// authorizes only the provider it is a subscription to, and the gateway takes everything
    /// that is neither subscription nor configured.
    #[must_use]
    pub fn authorizes_credential(self, source: Option<CredentialSource>) -> bool {
        let Some(selected) = source else {
            return false;
        };
        if selected == CredentialSource::HostManaged {
            return true;
        }
        match self {
            Self::Gateway => !matches!(
                selected,
                CredentialSource::ChatgptSubscription
                    | CredentialSource::GrokSubscription
                    | CredentialSource::Configured
            ),
            Self::Configured(_) => selected == CredentialSource::Configured,
            Self::Codex => selected == CredentialSource::ChatgptSubscription,
            Self::Grok => selected == CredentialSource::GrokSubscription,
        }
    }
}

impl Serialize for ProviderId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Gateway | Self::Codex | Self::Grok => serializer.serialize_str(self.label()),
            Self::Configured(identity) => {
                let binding = identity
                    .binding
                    .ok_or_else(|| serde::ser::Error::custom("configured provider is unbound"))?;
                ConfiguredProviderSaved {
                    name: identity.label(),
                    binding: crate::hex::encode(&binding),
                }
                .serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for ProviderId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        parse_saved(&value).map_err(|_| de::Error::custom(InvalidProviderBinding.to_string()))
    }
}

/// Validates a configured provider identifier.
pub fn validate_configured_id(id: &str) -> Result<(), ValidateConfiguredIdError> {
    if id.len() > MAX_CONFIGURED_PROVIDER_ID_BYTES {
        return Err(ValidateConfiguredIdError::LimitExceeded);
    }
    if id.is_empty() || !id.as_bytes()[0].is_ascii_alphabetic() {
        return Err(ValidateConfiguredIdError::InvalidProviderId);
    }
    if !id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(ValidateConfiguredIdError::InvalidProviderId);
    }
    for reserved in ["gateway", "codex", "grok"] {
        if id.eq_ignore_ascii_case(reserved) {
            return Err(ValidateConfiguredIdError::ReservedProviderId);
        }
    }
    Ok(())
}

/// Restores a provider from persisted JSON.
pub fn parse_saved(value: &serde_json::Value) -> Result<ProviderId, InvalidProviderBinding> {
    if let Some(text) = value.as_str() {
        let provider = ProviderId::parse(text).ok_or(InvalidProviderBinding)?;
        if matches!(provider, ProviderId::Configured(_)) {
            return Err(InvalidProviderBinding);
        }
        return Ok(provider);
    }
    let object = value.as_object().ok_or(InvalidProviderBinding)?;
    if object.len() != 2 {
        return Err(InvalidProviderBinding);
    }
    let name = object
        .get("name")
        .and_then(serde_json::Value::as_str)
        .ok_or(InvalidProviderBinding)?;
    let encoded = object
        .get("binding")
        .and_then(serde_json::Value::as_str)
        .ok_or(InvalidProviderBinding)?;
    if encoded.len() != 64 {
        return Err(InvalidProviderBinding);
    }
    let ProviderId::Configured(mut identity) = ProviderId::parse(name).ok_or(InvalidProviderBinding)? else {
        return Err(InvalidProviderBinding);
    };
    let binding = crate::hex::decode_array::<32>(encoded).ok_or(InvalidProviderBinding)?;
    identity.binding = Some(binding);
    Ok(ProviderId::Configured(identity))
}

#[derive(Serialize)]
struct ConfiguredProviderSaved<'a> {
    name: &'a str,
    binding: String,
}

/// Name-only key for provider-keyed stores whose lookups never inspect binding
/// identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameKey {
    bytes: [u8; MAX_CONFIGURED_PROVIDER_ID_BYTES],
    len: u8,
}

impl NameKey {
    /// Copies the name half of `provider`.
    #[must_use]
    pub fn from_provider(provider: ProviderId) -> Self {
        let name = provider.label();
        let mut bytes = [0; MAX_CONFIGURED_PROVIDER_ID_BYTES];
        bytes[..name.len()].copy_from_slice(name.as_bytes());
        Self {
            bytes,
            len: name
                .len()
                .try_into()
                .expect("provider label fits configured id storage"),
        }
    }

    /// Borrows the stored name.
    #[must_use]
    pub fn label(&self) -> &str {
        std::str::from_utf8(&self.bytes[..usize::from(self.len)]).expect("name key is utf-8")
    }

    /// Whether this key names `name`.
    #[must_use]
    pub fn eql_name(self, name: &str) -> bool {
        usize::from(self.len) == name.len() && self.label() == name
    }

    /// Whether this key names `provider`.
    #[must_use]
    pub fn eql_provider(self, provider: ProviderId) -> bool {
        self.eql_name(provider.label())
    }
}

/// A provider paired with the model selected on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSelection {
    /// The provider the model is reached through.
    pub provider: ProviderId,
    /// The model selected on that provider.
    pub model: ModelId,
}

/// One value held per built-in route, with no route left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ByProvider<T> {
    gateway: T,
    codex: T,
    grok: T,
}

impl<T> ByProvider<T> {
    /// One value per route, named in the order the routes are declared.
    pub const fn new(gateway: T, codex: T, grok: T) -> Self {
        Self { gateway, codex, grok }
    }

    /// What the built-in `provider` holds, when it is one of the three routes.
    pub const fn get(&self, provider: ProviderId) -> Option<&T> {
        match provider {
            ProviderId::Gateway => Some(&self.gateway),
            ProviderId::Codex => Some(&self.codex),
            ProviderId::Grok => Some(&self.grok),
            ProviderId::Configured(_) => None,
        }
    }

    /// What the built-in `provider` holds, to be replaced.
    pub const fn get_mut(&mut self, provider: ProviderId) -> Option<&mut T> {
        match provider {
            ProviderId::Gateway => Some(&mut self.gateway),
            ProviderId::Codex => Some(&mut self.codex),
            ProviderId::Grok => Some(&mut self.grok),
            ProviderId::Configured(_) => None,
        }
    }

    /// The same routes, each holding what `project` made of it.
    pub fn map<U>(self, mut project: impl FnMut(T) -> U) -> ByProvider<U> {
        ByProvider {
            gateway: project(self.gateway),
            codex: project(self.codex),
            grok: project(self.grok),
        }
    }

    /// Each built-in route paired with what it holds.
    pub fn iter(&self) -> impl Iterator<Item = (ProviderId, &T)> {
        [
            (ProviderId::Gateway, &self.gateway),
            (ProviderId::Codex, &self.codex),
            (ProviderId::Grok, &self.grok),
        ]
        .into_iter()
    }
}

impl<T> ByProvider<Option<T>> {
    /// Only the default route holds anything.
    pub fn gateway_only(gateway: T) -> Self {
        Self {
            gateway: Some(gateway),
            codex: None,
            grok: None,
        }
    }
}

/// Explicit host-admitted service authority; never a credential or a journal identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HostProviderBinding(crate::ContentHash);

impl HostProviderBinding {
    /// Accepts the digest of an independently admitted host registration.
    #[must_use]
    pub fn new(digest: crate::ContentHash) -> Self {
        Self(digest)
    }

    /// Opaque comparison bytes for the host/provider boundary.
    #[must_use]
    pub fn digest(self) -> crate::ContentHash {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use sha2::{Digest, Sha256};

    struct TestRegistry {
        bindings: HashMap<String, [u8; 32]>,
    }

    impl ConfiguredProviderRegistry for TestRegistry {
        fn binding_identity(&self, id: &str) -> Option<[u8; 32]> {
            self.bindings.get(id).copied()
        }
    }

    fn local_registry_binding() -> [u8; 32] {
        let mut hash = Sha256::new();
        hash_part(&mut hash, b"trogonai-configured-provider-v1");
        hash_part(&mut hash, b"local");
        hash_part(&mut hash, b"openai-chat-completions");
        hash_part(&mut hash, b"http://localhost:11434/v1");
        hash_part(&mut hash, b"none");
        hash.finalize().into()
    }

    fn hash_part(hasher: &mut Sha256, value: &[u8]) {
        hasher.update((value.len() as u64).to_le_bytes());
        hasher.update(value);
    }

    #[test]
    fn provider_parsing_recognizes_builtins_and_validated_configured_names() {
        assert_eq!(ProviderId::parse("gateway"), Some(ProviderId::Gateway));
        assert_eq!(ProviderId::parse("CODEX"), Some(ProviderId::Codex));
        assert_eq!(ProviderId::parse("GROK"), Some(ProviderId::Grok));
        assert!(matches!(
            ProviderId::parse("openai-codex"),
            Some(ProviderId::Configured(_))
        ));
        assert_eq!(ProviderId::parse("bad/name"), None);
        assert_eq!(ProviderId::parse(""), None);
    }

    #[test]
    fn an_explicit_provider_is_authorized_only_by_its_own_credential_origin() {
        assert!(ProviderId::Gateway.authorizes_credential(Some(CredentialSource::AiGatewayApiKey)));
        assert!(ProviderId::Gateway.authorizes_credential(Some(CredentialSource::TrogonaiLogin)));
        assert!(!ProviderId::Gateway.authorizes_credential(Some(CredentialSource::ChatgptSubscription)));
        assert!(ProviderId::Codex.authorizes_credential(Some(CredentialSource::ChatgptSubscription)));
        assert!(!ProviderId::Codex.authorizes_credential(Some(CredentialSource::AiGatewayApiKey)));
        assert!(!ProviderId::Codex.authorizes_credential(None));
        assert!(ProviderId::Grok.authorizes_credential(Some(CredentialSource::GrokSubscription)));
        assert!(!ProviderId::Grok.authorizes_credential(Some(CredentialSource::ChatgptSubscription)));
        assert!(!ProviderId::Gateway.authorizes_credential(Some(CredentialSource::GrokSubscription)));
    }

    #[test]
    fn a_host_managed_credential_authorizes_every_provider() {
        assert!(ProviderId::Gateway.authorizes_credential(Some(CredentialSource::HostManaged)));
        assert!(ProviderId::Codex.authorizes_credential(Some(CredentialSource::HostManaged)));
        assert!(ProviderId::Grok.authorizes_credential(Some(CredentialSource::HostManaged)));
    }

    #[test]
    fn no_provider_is_authorized_by_the_absence_of_a_credential_source() {
        assert!(!ProviderId::Gateway.authorizes_credential(None));
        assert!(!ProviderId::Codex.authorizes_credential(None));
        assert!(!ProviderId::Grok.authorizes_credential(None));
    }

    #[test]
    fn a_provider_selection_round_trips_through_serde_json() {
        let selection = ProviderSelection {
            provider: ProviderId::Gateway,
            model: ModelId::new("anthropic/claude-opus-4").unwrap(),
        };
        let json = serde_json::to_string(&selection).unwrap();
        let restored: ProviderSelection = serde_json::from_str(&json).unwrap();
        assert_eq!(selection, restored);
    }

    #[test]
    fn every_route_holds_its_own_value_and_answers_for_no_other() {
        let mut routes = ByProvider::new(1, 2, 3);

        assert_eq!(*routes.get(ProviderId::Gateway).expect("gateway slot"), 1);
        assert_eq!(*routes.get(ProviderId::Codex).expect("codex slot"), 2);
        assert_eq!(*routes.get(ProviderId::Grok).expect("grok slot"), 3);

        *routes.get_mut(ProviderId::Grok).expect("grok slot") = 30;
        assert_eq!(*routes.get(ProviderId::Grok).expect("grok slot"), 30);
        assert_eq!(*routes.get(ProviderId::Gateway).expect("gateway slot"), 1);
        assert_eq!(routes.map(|value| value * 2), ByProvider::new(2, 4, 60));
    }

    #[test]
    fn a_gateway_only_set_leaves_the_subscription_routes_empty() {
        let routes = ByProvider::gateway_only("wired");

        assert_eq!(*routes.get(ProviderId::Gateway).expect("gateway slot"), Some("wired"));
        assert_eq!(*routes.get(ProviderId::Codex).expect("codex slot"), None);
        assert_eq!(*routes.get(ProviderId::Grok).expect("grok slot"), None);
        assert_eq!(routes.iter().filter(|(_, provider)| provider.is_some()).count(), 1);
    }

    #[test]
    fn configured_provider_identity_serializes_its_binding_and_rejects_rebinding() {
        let registry = TestRegistry {
            bindings: HashMap::from([("local".to_owned(), local_registry_binding())]),
        };
        let unbound = ProviderId::parse("local").expect("local is valid");
        let bound = unbound.bind(&registry).expect("registry defines local");
        assert!(!unbound.same_authority(bound));

        let json = serde_json::to_string(&bound).expect("bound provider encodes");
        let decoded: ProviderId = serde_json::from_str(&json).expect("saved provider decodes");
        assert_eq!(decoded.label(), "local");
        assert!(decoded.same_authority(bound));

        let ProviderId::Configured(mut changed) = bound else {
            panic!("bound provider stays configured");
        };
        changed.binding.as_mut().expect("bound identity")[0] ^= 1;
        assert_eq!(
            ProviderId::Configured(changed).bind(&registry),
            Err(ProviderBindError::ConfiguredProviderChanged)
        );
        assert_eq!(
            bound.bind(&TestRegistry {
                bindings: HashMap::new(),
            }),
            Err(ProviderBindError::UnknownConfiguredProvider)
        );
        assert!(parse_saved(&serde_json::Value::String("local".into())).is_err());
        assert!(!ProviderId::Gateway.authorizes_credential(Some(CredentialSource::Configured)));
        assert!(!bound.authorizes_credential(Some(CredentialSource::AiGatewayApiKey)));
    }

    #[test]
    fn provider_equality_compares_tags_before_names_and_name_keys_stay_name_only() {
        let gateway = ProviderId::parse("gateway").expect("gateway");
        let codex = ProviderId::parse("codex").expect("codex");
        assert!(gateway.eql(ProviderId::parse("gateway").expect("gateway")));
        assert!(!gateway.eql(codex));
        assert!(!gateway.eql(ProviderId::parse("local").expect("local")));

        let local = ProviderId::parse("local").expect("local");
        assert!(local.eql(ProviderId::parse("local").expect("local")));
        assert!(!local.eql(ProviderId::parse("remote").expect("remote")));

        let key = NameKey::from_provider(local);
        assert_eq!(key.label(), "local");
        assert!(key.eql_name("local"));
        assert!(!key.eql_name("remote"));
        assert!(key.eql_provider(local));
        assert!(!key.eql_provider(ProviderId::parse("remote").expect("remote")));

        let builtin_key = NameKey::from_provider(gateway);
        assert!(builtin_key.eql_name("gateway"));
        assert!(builtin_key.eql_provider(gateway));
    }
}
