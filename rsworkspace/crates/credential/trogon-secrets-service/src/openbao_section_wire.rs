use serde::Deserialize;

/// The `[openbao]` section as written. The AppRole secret id is named by the
/// file that holds it, never written here.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OpenBaoSectionWire {
    pub(crate) address: Option<String>,
    pub(crate) mount: Option<String>,
    pub(crate) role_id: Option<String>,
    pub(crate) secret_id_file: Option<String>,
    pub(crate) child_token_role: Option<String>,
    pub(crate) child_token_policy: Option<String>,
}
