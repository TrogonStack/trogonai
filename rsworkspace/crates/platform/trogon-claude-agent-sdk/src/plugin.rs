use std::path::PathBuf;

use serde::Serialize;

/// A local plugin directory, TypeScript's `{ type: 'local', path, skipMcpDiscovery? }` and Python's
/// `SdkPluginConfig`. Python has no `skipMcpDiscovery`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalPlugin {
    pub path: PathBuf,
    /// TypeScript's `skipMcpDiscovery`: `Some(true)` passes `--plugin-dir-no-mcp`. An explicit `Some(false)` is
    /// still sent to `initialize` as `false`, as the object the caller built is forwarded as is.
    pub skip_mcp_discovery: Option<bool>,
}

impl LocalPlugin {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            skip_mcp_discovery: None,
        }
    }
}

/// TypeScript's `pluginDelivery`. Python has no such option and always passes `--plugin-dir`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PluginDelivery {
    /// `--plugin-dir` flags on the command line.
    #[default]
    Argv,
    /// The `initialize` request carries the plugins and the CLI is started with `--await-initialize`. With no
    /// plugins this behaves like `Argv`.
    Initialize,
}

/// The `plugins` entries of the `initialize` request, in the object shape the TypeScript SDK sends.
#[derive(Serialize)]
pub(crate) struct PluginPayload<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    path: &'a std::path::Path,
    #[serde(rename = "skipMcpDiscovery", skip_serializing_if = "Option::is_none")]
    skip_mcp_discovery: Option<bool>,
}

impl<'a> From<&'a LocalPlugin> for PluginPayload<'a> {
    fn from(plugin: &'a LocalPlugin) -> Self {
        Self {
            kind: "local",
            path: &plugin.path,
            skip_mcp_discovery: plugin.skip_mcp_discovery,
        }
    }
}
