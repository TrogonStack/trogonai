//! How a status answer is asked for.

use serde::{Deserialize, Serialize};

/// Which rendering of a snapshot the caller wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    /// Lines meant for a person.
    Text,
    /// One JSON document meant for a program.
    Json,
}

impl OutputFormat {
    /// The name this format is written as on a command line.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
        }
    }
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}
