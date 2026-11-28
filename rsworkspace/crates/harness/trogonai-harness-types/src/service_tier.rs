//! Paid latency preferences and provider-confirmed serving are independent facts.

use serde::{Deserialize, Serialize};

/// The user's explicit premium latency choice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ServiceTierPreference {
    /// Preserve ordinary serving and the separate Fast choice.
    #[default]
    Off,
    /// Opt in to Ultrafast pricing where the route can verify support.
    Ultrafast,
}

impl ServiceTierPreference {
    /// The boolean used by upstream settings and session formats.
    #[must_use]
    pub const fn enabled(self) -> bool {
        matches!(self, Self::Ultrafast)
    }

    /// Converts an explicit persisted or process choice.
    #[must_use]
    pub const fn from_enabled(enabled: bool) -> Self {
        if enabled { Self::Ultrafast } else { Self::Off }
    }

    /// Parses the upstream environment boolean vocabulary.
    #[must_use]
    pub fn parse_override(raw: &str) -> Option<Self> {
        let raw = raw.trim_matches([' ', '\t', '\r', '\n']);
        if ["1", "true", "on", "yes"]
            .iter()
            .any(|value| raw.eq_ignore_ascii_case(value))
        {
            Some(Self::Ultrafast)
        } else if ["0", "false", "off", "no"]
            .iter()
            .any(|value| raw.eq_ignore_ascii_case(value))
        {
            Some(Self::Off)
        } else {
            None
        }
    }
}

/// What a catalog has established about premium latency.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PremiumLatencyCapability {
    /// No authoritative catalog answer exists.
    #[default]
    Unknown,
    /// The catalog does not offer Ultrafast.
    Unsupported,
    /// The catalog offers Ultrafast.
    Ultrafast,
}

/// The processing tier the provider actually confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServedServiceTier {
    /// Standard processing.
    Standard,
    /// Flex processing.
    Flex,
    /// Priority processing, including the Fast alias.
    Priority,
    /// Ultrafast processing.
    Ultrafast,
}

impl ServedServiceTier {
    /// Parses the upstream provider vocabulary without inferring from requests.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "default" | "standard" => Some(Self::Standard),
            "priority" | "fast" => Some(Self::Priority),
            "flex" => Some(Self::Flex),
            "ultrafast" => Some(Self::Ultrafast),
            _ => None,
        }
    }
}

/// Evidence about an opt-in request's actual serving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PremiumServingStatus {
    /// No premium request was made.
    NotRequested,
    /// The provider explicitly confirmed Ultrafast.
    Confirmed,
    /// The provider explicitly reported another tier.
    Downgraded(ServedServiceTier),
    /// No recognized provider tier was reported.
    Unconfirmed,
}

impl PremiumServingStatus {
    /// Compares the user's request with provider evidence.
    #[must_use]
    pub const fn resolve(preference: ServiceTierPreference, served: Option<ServedServiceTier>) -> Self {
        match (preference, served) {
            (ServiceTierPreference::Off, _) => Self::NotRequested,
            (_, Some(ServedServiceTier::Ultrafast)) => Self::Confirmed,
            (_, Some(tier)) => Self::Downgraded(tier),
            (_, None) => Self::Unconfirmed,
        }
    }
}

/// A session's durable choice and this process's temporary override.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServiceTierSelection {
    /// The choice saved with the session.
    pub persisted: ServiceTierPreference,
    /// A temporary choice that must never replace the saved baseline.
    pub process_override: Option<ServiceTierPreference>,
}

impl ServiceTierSelection {
    /// The effective choice, including explicit off.
    #[must_use]
    pub fn effective(self) -> ServiceTierPreference {
        self.process_override.unwrap_or(self.persisted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premium_latency_requires_an_explicit_valid_opt_in() {
        assert_eq!(ServiceTierPreference::default(), ServiceTierPreference::Off);
        for raw in ["1", "true", "ON", " yes\t"] {
            assert_eq!(
                ServiceTierPreference::parse_override(raw),
                Some(ServiceTierPreference::Ultrafast)
            );
        }
        for raw in ["0", "false", "OFF", " no\r\n"] {
            assert_eq!(
                ServiceTierPreference::parse_override(raw),
                Some(ServiceTierPreference::Off)
            );
        }
        for raw in ["", "premium", "\u{a0}true", "true\u{b}"] {
            assert_eq!(ServiceTierPreference::parse_override(raw), None);
        }
    }

    #[test]
    fn a_process_override_never_changes_the_durable_baseline() {
        for baseline in [ServiceTierPreference::Off, ServiceTierPreference::Ultrafast] {
            let temporary = ServiceTierPreference::from_enabled(!baseline.enabled());
            let selection = ServiceTierSelection {
                persisted: baseline,
                process_override: Some(temporary),
            };
            assert_eq!(selection.effective(), temporary);
            assert_eq!(selection.persisted, baseline);
        }
    }

    #[test]
    fn serving_is_confirmed_only_by_the_provider() {
        let asked = ServiceTierPreference::Ultrafast;
        assert_eq!(
            PremiumServingStatus::resolve(asked, None),
            PremiumServingStatus::Unconfirmed
        );
        assert_eq!(
            PremiumServingStatus::resolve(asked, ServedServiceTier::parse("unknown")),
            PremiumServingStatus::Unconfirmed
        );
        assert_eq!(
            PremiumServingStatus::resolve(asked, Some(ServedServiceTier::Priority)),
            PremiumServingStatus::Downgraded(ServedServiceTier::Priority)
        );
        assert_eq!(
            PremiumServingStatus::resolve(asked, Some(ServedServiceTier::Ultrafast)),
            PremiumServingStatus::Confirmed
        );
    }
}
