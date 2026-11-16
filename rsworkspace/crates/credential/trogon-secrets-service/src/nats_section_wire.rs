use serde::Deserialize;

/// The `[nats]` section as written.
///
/// The authentication secrets are read as any TOML value, so a secret written
/// as a number or a table is refused by this crate, which never quotes it,
/// rather than by a type error that would.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NatsSectionWire {
    pub(crate) url: Option<String>,
    pub(crate) creds: Option<String>,
    pub(crate) nkey: Option<toml::Value>,
    pub(crate) user: Option<String>,
    pub(crate) password: Option<toml::Value>,
    pub(crate) token: Option<toml::Value>,
    pub(crate) jwt: Option<toml::Value>,
    pub(crate) seed: Option<toml::Value>,
    pub(crate) tls_cert: Option<String>,
    pub(crate) tls_key: Option<String>,
    pub(crate) tls_ca: Option<String>,
}
