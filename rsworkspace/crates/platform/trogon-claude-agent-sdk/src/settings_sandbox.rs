//! Upstream names for the sandbox settings.
//!
//! TypeScript declares `SandboxSettings` and its parts as Zod inferences that the declaration
//! generator cannot read, but the same shapes are the `sandbox` property of `Settings`, which it
//! does generate (16 keys, open object). These aliases give them the upstream names. Python's
//! `SandboxSettings` and `SandboxNetworkConfig` TypedDicts use a subset of those keys with the same
//! spelling, so a Python sandbox dict decodes and encodes through these types unchanged.

pub use crate::upstream::settings::SettingsSandbox as SandboxSettings;
pub use crate::upstream::settings::SettingsSandboxCredentials as SandboxCredentialsConfig;
pub use crate::upstream::settings::SettingsSandboxFilesystem as SandboxFilesystemConfig;
pub use crate::upstream::settings::SettingsSandboxNetwork as SandboxNetworkConfig;

/// Command pattern to violation substrings, `SandboxSettings['ignoreViolations']`.
pub type SandboxIgnoreViolations = indexmap::IndexMap<String, Vec<String>>;
