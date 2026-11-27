use serde::Deserialize;

/// The public URL Daytona serves a sandbox port on, and the token that opens it.
#[derive(Debug, Clone, Deserialize)]
pub struct PortPreview {
    pub url: String,
    #[serde(default)]
    pub token: Option<String>,
}
