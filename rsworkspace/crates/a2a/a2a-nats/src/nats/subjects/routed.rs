//! The layout gateway ingress, agent delivery and task event subjects share:
//! `{prefix}.v1.{family}.{route}.{kind}.{id}.{terminal…}`, where `{kind}.{id}` is
//! the caller's principal position (ADR#0082).

use crate::a2a_prefix::A2aPrefix;

/// The tokens of a routed subject, split but not yet validated.
pub(super) struct RoutedTokens<'a> {
    pub(super) route: &'a str,
    pub(super) kind: &'a str,
    pub(super) id: &'a str,
    pub(super) terminal: &'a str,
}

/// Why a subject did not split into the routed layout.
pub(super) enum RoutedSplitError {
    ForeignFamily,
    MissingTokens,
}

impl<'a> RoutedTokens<'a> {
    pub(super) fn split(prefix: &A2aPrefix, family: &str, subject: &'a str) -> Result<Self, RoutedSplitError> {
        let rest = subject
            .strip_prefix(prefix.as_str())
            .and_then(|rest| rest.strip_prefix(".v1."))
            .and_then(|rest| rest.strip_prefix(family))
            .and_then(|rest| rest.strip_prefix('.'))
            .ok_or(RoutedSplitError::ForeignFamily)?;
        let mut tokens = rest.splitn(4, '.');
        match (tokens.next(), tokens.next(), tokens.next(), tokens.next()) {
            (Some(route), Some(kind), Some(id), Some(terminal)) => Ok(Self {
                route,
                kind,
                id,
                terminal,
            }),
            _ => Err(RoutedSplitError::MissingTokens),
        }
    }
}
