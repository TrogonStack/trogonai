use serde::{Deserialize, Serialize};

/// The directories the macOS mandatory protections are laid out beneath.
///
/// It serializes as the snake case strings `write_roots` and `working_directory`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectedNamesAnchor {
    /// Beneath every writable root, wherever the command runs.
    #[default]
    WriteRoots,
    /// Beneath the working directory only.
    WorkingDirectory,
}

impl ProtectedNamesAnchor {
    /// Reports whether the anchor is the default, so the wire can leave it out.
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}
