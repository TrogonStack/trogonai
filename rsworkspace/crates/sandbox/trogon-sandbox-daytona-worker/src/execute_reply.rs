use serde::Deserialize;

/// What a toolbox `process/execute` call answers with.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteReply {
    #[serde(default)]
    pub exit_code: i32,
    #[serde(default)]
    pub result: String,
}
