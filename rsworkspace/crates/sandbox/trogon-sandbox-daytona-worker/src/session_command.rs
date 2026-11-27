use serde::Serialize;

/// Body of a toolbox call that starts a command inside a session.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionCommand {
    pub(crate) command: String,
    pub(crate) run_async: bool,
    /// Keeps the session from echoing the input lines it is fed back into its own output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) suppress_input_echo: Option<bool>,
}
