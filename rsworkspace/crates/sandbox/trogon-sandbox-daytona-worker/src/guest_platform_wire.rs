use serde::Deserialize;

/// What the platform probe prints inside the sandbox, as it arrives.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GuestPlatformWire {
    pub(crate) platform: String,
    pub(crate) architecture: String,
    pub(crate) node_major: u32,
    pub(crate) node_minor: u32,
}
