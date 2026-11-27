//! Settings keys that load because this runtime already behaves as the key
//! asks, or because the tool the key points at is not part of this runtime.
//! A value that would ask for something else is still refused, and a value
//! this runtime does not honor loads with a warning.

use std::time::Duration;

use serde::Deserialize;
use serde::de::{self, Deserializer};
use trogon_sandbox_contract::AmbiguousTargets;
use trogon_sandbox_protocol::HostMatch as CoreHostMatch;

use super::constants::{MAX_SEARCH_DEPTH, MIN_SEARCH_DEPTH, REFUSE};
use super::issue_wording;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RipgrepSettings {
    #[allow(dead_code)]
    command: String,
    #[serde(default, deserialize_with = "super::present")]
    #[allow(dead_code)]
    args: Option<Vec<String>>,
    #[serde(default, deserialize_with = "super::present")]
    #[allow(dead_code)]
    argv0: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SeccompSettings {
    #[serde(default, deserialize_with = "super::present")]
    #[allow(dead_code)]
    apply_path: Option<String>,
    #[serde(default, deserialize_with = "super::present")]
    #[allow(dead_code)]
    argv0: Option<String>,
}

#[derive(Debug)]
pub(super) struct SearchDepth(u8);

impl SearchDepth {
    pub(super) fn get(&self) -> u8 {
        self.0
    }
}

impl<'de> Deserialize<'de> for SearchDepth {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let depth = issue_wording::integer(
            deserializer,
            Some(i64::from(MIN_SEARCH_DEPTH)),
            Some(i64::from(MAX_SEARCH_DEPTH)),
        )?;
        u8::try_from(depth).map(Self).map_err(de::Error::custom)
    }
}

#[derive(Debug)]
pub(super) struct HostMatch(pub(super) CoreHostMatch);

impl<'de> Deserialize<'de> for HostMatch {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let required = bool::deserialize(deserializer)?;
        Ok(Self(if required {
            CoreHostMatch::Required
        } else {
            CoreHostMatch::NotRequired
        }))
    }
}

#[derive(Debug)]
pub(super) struct AmbiguousTarget(pub(super) AmbiguousTargets);

impl<'de> Deserialize<'de> for AmbiguousTarget {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        issue_wording::one_of(
            deserializer,
            &[
                (REFUSE, AmbiguousTargets::Refuse),
                ("asSpelled", AmbiguousTargets::AsSpelled),
                ("normalize", AmbiguousTargets::Normalize),
            ],
        )
        .map(Self)
    }
}

#[derive(Debug)]
pub(super) struct HandshakeTimeoutMs(pub(super) Duration);

impl<'de> Deserialize<'de> for HandshakeTimeoutMs {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let millis = issue_wording::integer(deserializer, Some(100), Some(600_000))?;
        let millis = u64::try_from(millis).map_err(de::Error::custom)?;
        Ok(Self(Duration::from_millis(millis)))
    }
}
