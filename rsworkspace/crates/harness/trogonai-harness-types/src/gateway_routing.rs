//! Which gateway backends a request may be served by, and which one served it.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ProviderId;

/// The most providers one routing list may name.
pub const MAX_PROVIDER_ORDER_ENTRIES: usize = 8;

/// The longest provider slug a routing list may hold.
pub const MAX_PROVIDER_SLUG_BYTES: usize = 64;

/// The longest served-provider name the gateway may report back.
pub const MAX_SERVED_PROVIDER_BYTES: usize = 128;

/// Why a routing list was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProviderRoutingError {
    /// A slug is empty, too long, or holds something other than letters,
    /// digits and dashes after a leading letter or digit.
    #[error(
        "a gateway provider is 1 to {MAX_PROVIDER_SLUG_BYTES} letters, digits or dashes, starting with a letter or digit"
    )]
    Slug,
    /// The list names more providers than the gateway is asked to try.
    #[error("a gateway provider list names at most {MAX_PROVIDER_ORDER_ENTRIES} providers")]
    TooMany,
    /// The list names one provider twice.
    #[error("`{0}` is named twice in the gateway provider list")]
    Duplicate(String),
}

/// One gateway provider identifier, such as `bedrock` or `vertexAnthropic`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProviderSlug(String);

impl ProviderSlug {
    /// The slug `raw` spells, exactly as written.
    ///
    /// # Errors
    ///
    /// [`ProviderRoutingError::Slug`] when `raw` is not a gateway provider
    /// identifier. Case is kept, because the gateway's own slugs mix it.
    pub fn parse(raw: &str) -> Result<Self, ProviderRoutingError> {
        let bytes = raw.as_bytes();
        let valid = !bytes.is_empty()
            && bytes.len() <= MAX_PROVIDER_SLUG_BYTES
            && bytes[0].is_ascii_alphanumeric()
            && bytes.iter().all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-');
        if valid {
            Ok(Self(raw.to_owned()))
        } else {
            Err(ProviderRoutingError::Slug)
        }
    }

    /// The slug as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderSlug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Gateway providers in the order they should be tried.
///
/// An empty order is a real value and not the absence of one: a settings file
/// that writes `[]` clears a list a weaker file set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderOrder(Vec<ProviderSlug>);

impl ProviderOrder {
    /// An order that names nobody, leaving the choice to the gateway.
    #[must_use]
    pub const fn none() -> Self {
        Self(Vec::new())
    }

    /// The order these slugs spell.
    ///
    /// # Errors
    ///
    /// Refuses a slug that is not a provider identifier, a list longer than
    /// [`MAX_PROVIDER_ORDER_ENTRIES`], or a provider named twice.
    pub fn from_slugs<'a>(slugs: impl IntoIterator<Item = &'a str>) -> Result<Self, ProviderRoutingError> {
        let mut order = Vec::new();
        for raw in slugs {
            let slug = ProviderSlug::parse(raw)?;
            if order.len() >= MAX_PROVIDER_ORDER_ENTRIES {
                return Err(ProviderRoutingError::TooMany);
            }
            if order.contains(&slug) {
                return Err(ProviderRoutingError::Duplicate(slug.0));
            }
            order.push(slug);
        }
        Ok(Self(order))
    }

    /// The order a comma-separated list such as `"azure, anthropic"` spells,
    /// or `None` when it names nobody.
    ///
    /// # Errors
    ///
    /// The same as [`Self::from_slugs`].
    pub fn parse_list(raw: &str) -> Result<Option<Self>, ProviderRoutingError> {
        let order = Self::from_slugs(
            raw.split(',')
                .map(|token| token.trim_matches([' ', '\t', '\r', '\n']))
                .filter(|token| !token.is_empty()),
        )?;
        Ok((!order.is_empty()).then_some(order))
    }

    /// Whether the order names nobody.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The providers, first preference first.
    #[must_use]
    pub fn slugs(&self) -> &[ProviderSlug] {
        &self.0
    }
}

/// Which gateway providers a request may be served by.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderRouting {
    order: ProviderOrder,
    strict: bool,
}

impl ProviderRouting {
    /// Routing that leaves every choice to the gateway.
    #[must_use]
    pub const fn gateway_default() -> Self {
        Self {
            order: ProviderOrder::none(),
            strict: false,
        }
    }

    /// Routing that tries `order` first, and only `order` when `strict`.
    #[must_use]
    pub const fn new(order: ProviderOrder, strict: bool) -> Self {
        Self { order, strict }
    }

    /// The providers to try, first preference first.
    #[must_use]
    pub const fn order(&self) -> &ProviderOrder {
        &self.order
    }

    /// Whether the order is a restriction rather than a preference.
    #[must_use]
    pub const fn strict(&self) -> bool {
        self.strict
    }

    /// Whether a request carrying this routing says anything about it.
    #[must_use]
    pub fn is_gateway_default(&self) -> bool {
        self.order.is_empty()
    }

    /// The routing a request on `provider` carries.
    ///
    /// Routing is a gateway control, so a turn on any other route carries none
    /// rather than being refused for a setting it cannot honour.
    #[must_use]
    pub fn for_provider(&self, provider: ProviderId) -> Self {
        if provider == ProviderId::Gateway {
            self.clone()
        } else {
            Self::gateway_default()
        }
    }
}

/// What one launch's flags say about routing, on top of what settings said.
///
/// Flags shape the launch they are given to and are never written back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderRoutingOverride {
    order: Option<ProviderOrder>,
    strict: Option<bool>,
}

impl ProviderRoutingOverride {
    /// Replaces any order an earlier flag gave.
    pub fn set_order(&mut self, order: ProviderOrder) {
        self.order = Some(order);
    }

    /// Records a strictness flag.
    ///
    /// # Errors
    ///
    /// [`ConflictingStrictness`] when an earlier flag said the opposite.
    pub fn set_strict(&mut self, strict: bool) -> Result<(), ConflictingStrictness> {
        if self.strict.is_some_and(|current| current != strict) {
            return Err(ConflictingStrictness);
        }
        self.strict = Some(strict);
        Ok(())
    }

    /// The order the flags gave, when they gave one.
    #[must_use]
    pub const fn order(&self) -> Option<&ProviderOrder> {
        self.order.as_ref()
    }

    /// The strictness the flags gave, when they gave one.
    #[must_use]
    pub const fn strict(&self) -> Option<bool> {
        self.strict
    }

    /// Whether the flags said anything about routing.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.order.is_none() && self.strict.is_none()
    }

    /// `base` with whatever the flags stated laid over it.
    #[must_use]
    pub fn applied_to(&self, base: &ProviderRouting) -> ProviderRouting {
        ProviderRouting::new(
            self.order.clone().unwrap_or_else(|| base.order().clone()),
            self.strict.unwrap_or(base.strict()),
        )
    }
}

/// A launch asked for strict and non-strict routing at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("--provider-strict and --no-provider-strict cannot be used together")]
pub struct ConflictingStrictness;

/// Why a served-provider name was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("a served provider is 1 to {MAX_SERVED_PROVIDER_BYTES} visible ASCII characters with no spaces")]
pub struct InvalidServedProvider;

/// The gateway provider that actually served a completion.
///
/// The gateway reports it for display; a malformed report is dropped rather
/// than failing the turn, which is why the check is looser than a
/// [`ProviderSlug`] and only keeps the name printable and bounded.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ServedProvider(String);

impl ServedProvider {
    /// The served provider `raw` names.
    ///
    /// # Errors
    ///
    /// [`InvalidServedProvider`] when `raw` is empty, longer than
    /// [`MAX_SERVED_PROVIDER_BYTES`], or holds anything but visible ASCII.
    pub fn parse(raw: &str) -> Result<Self, InvalidServedProvider> {
        let valid = !raw.is_empty()
            && raw.len() <= MAX_SERVED_PROVIDER_BYTES
            && raw.bytes().all(|byte| (0x21..=0x7e).contains(&byte));
        if valid {
            Ok(Self(raw.to_owned()))
        } else {
            Err(InvalidServedProvider)
        }
    }

    /// The name as reported.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ServedProvider {
    type Error = InvalidServedProvider;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ServedProvider> for String {
    fn from(value: ServedProvider) -> Self {
        value.0
    }
}

impl fmt::Display for ServedProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_comma_list_is_trimmed_and_keeps_the_gateways_mixed_case_slugs() {
        let order = ProviderOrder::parse_list(" azure, vertexAnthropic ,bedrock")
            .expect("valid list")
            .expect("names providers");
        let slugs: Vec<&str> = order.slugs().iter().map(ProviderSlug::as_str).collect();
        assert_eq!(slugs, ["azure", "vertexAnthropic", "bedrock"]);
    }

    #[test]
    fn a_blank_list_names_nobody() {
        assert_eq!(ProviderOrder::parse_list("  ,  "), Ok(None));
    }

    #[test]
    fn a_bad_slug_a_repeat_or_too_many_providers_are_refused() {
        assert_eq!(
            ProviderOrder::parse_list("azure,Bad Slug"),
            Err(ProviderRoutingError::Slug)
        );
        assert_eq!(ProviderOrder::parse_list("-azure"), Err(ProviderRoutingError::Slug));
        assert_eq!(
            ProviderOrder::parse_list("azure,azure"),
            Err(ProviderRoutingError::Duplicate("azure".to_owned()))
        );
        assert_eq!(
            ProviderOrder::parse_list("a,b,c,d,e,f,g,h,i"),
            Err(ProviderRoutingError::TooMany)
        );
        let long = "a".repeat(MAX_PROVIDER_SLUG_BYTES + 1);
        assert_eq!(ProviderSlug::parse(&long), Err(ProviderRoutingError::Slug));
    }

    #[test]
    fn routing_only_rides_on_gateway_turns() {
        let routing = ProviderRouting::new(ProviderOrder::from_slugs(["bedrock"]).expect("valid"), true);
        assert_eq!(routing.for_provider(ProviderId::Gateway), routing);
        assert!(routing.for_provider(ProviderId::Codex).is_gateway_default());
    }

    #[test]
    fn launch_flags_replace_only_what_they_state() {
        let base = ProviderRouting::new(ProviderOrder::from_slugs(["azure"]).expect("valid"), true);
        let mut flags = ProviderRoutingOverride::default();
        assert_eq!(flags.applied_to(&base), base);

        flags.set_order(ProviderOrder::from_slugs(["bedrock"]).expect("valid"));
        assert_eq!(
            flags.applied_to(&base),
            ProviderRouting::new(ProviderOrder::from_slugs(["bedrock"]).expect("valid"), true)
        );
        assert_eq!(flags.set_strict(false), Ok(()));
        assert_eq!(flags.set_strict(true), Err(ConflictingStrictness));
        assert!(!flags.applied_to(&base).strict());
    }

    #[test]
    fn a_served_provider_is_bounded_and_printable() {
        assert_eq!(
            ServedProvider::parse("bedrock").map(|p| p.as_str().to_owned()),
            Ok("bedrock".to_owned())
        );
        assert!(ServedProvider::parse("").is_err());
        assert!(ServedProvider::parse("has space").is_err());
        assert!(ServedProvider::parse(&"x".repeat(MAX_SERVED_PROVIDER_BYTES + 1)).is_err());
    }
}
