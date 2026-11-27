use serde::Deserialize;

/// The body of Daytona's SSH access reply, as it arrives.
#[derive(Deserialize)]
pub(crate) struct DaytonaSshGrantWire {
    pub(crate) token: String,
    #[serde(rename = "sshCommand")]
    pub(crate) ssh_command: String,
    #[serde(default, rename = "expiresAt")]
    pub(crate) expires_at: Option<String>,
}
