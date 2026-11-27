use std::collections::BTreeMap;

use serde::Serialize;

/// The body of `POST /v1/sandboxes/{id}/commands`.
#[derive(Serialize)]
pub(crate) struct CommandBody<'a> {
    pub(crate) command: &'a [String],
    pub(crate) timeout: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) env: &'a BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cwd: Option<&'a str>,
}
