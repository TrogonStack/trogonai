use serde::{Deserialize, Serialize};

/// Whether a leased container or VM runs the image's own entrypoint on boot.
///
/// `Bare` starts the sandbox with no entrypoint so the caller drives execution explicitly, for example through an SSH
/// exec gateway. It is an opt-in mode rather than a change of the default. It serializes as the JSON strings
/// `entrypoint_default` and `bare`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootMode {
    #[default]
    EntrypointDefault,
    Bare,
}
