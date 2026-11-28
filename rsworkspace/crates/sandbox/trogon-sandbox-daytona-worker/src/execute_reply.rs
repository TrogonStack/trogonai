use serde::Deserialize;

/// What a toolbox `process/execute` call answers with.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteReply {
    /// Required: a reply without an exit code would otherwise read as a clean exit.
    pub exit_code: i32,
    #[serde(default)]
    pub result: String,
}
