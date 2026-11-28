//! Which stream of builds the runtime follows.

use serde::{Deserialize, Serialize};

/// A stream of builds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// Released builds.
    #[default]
    Stable,
    /// Builds cut from the trunk.
    Dev,
}

impl Channel {
    /// The channel a configured value names.
    ///
    /// Returns `None` rather than a default for anything else: silently following stable when a
    /// person asked for something the runtime did not recognize is how a machine ends up on a channel
    /// nobody chose.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        if raw.eq_ignore_ascii_case("stable") {
            return Some(Self::Stable);
        }
        if raw.eq_ignore_ascii_case("dev") {
            return Some(Self::Dev);
        }
        None
    }

    /// The name this channel is written as.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Dev => "dev",
        }
    }
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_channel_written_any_way_a_person_might_write_it_is_still_that_channel() {
        assert_eq!(Channel::parse("stable"), Some(Channel::Stable));
        assert_eq!(Channel::parse("Dev"), Some(Channel::Dev));
        assert_eq!(Channel::parse("DEV"), Some(Channel::Dev));
    }

    #[test]
    fn a_channel_trogonai_does_not_know_is_refused_rather_than_rounded_to_one_it_does() {
        assert_eq!(Channel::parse("nightly"), None);
        assert_eq!(Channel::parse(""), None);
        assert_eq!(Channel::parse(" dev"), None);
    }

    #[test]
    fn the_name_a_channel_is_reported_as_is_the_name_it_parses_from() {
        for channel in [Channel::Stable, Channel::Dev] {
            assert_eq!(Channel::parse(channel.label()), Some(channel));
        }
    }

    #[test]
    fn a_build_nobody_said_anything_about_is_a_released_one() {
        assert_eq!(Channel::default(), Channel::Stable);
    }
}
