use serde::Deserialize;

/// The toolbox proxy Daytona routes a sandbox's toolbox calls through.
#[derive(Debug, Deserialize)]
pub(crate) struct ProxyUrl {
    pub(crate) url: String,
}
