use serde::{Deserialize, Serialize};

use crate::{
    CredentialRequest, Extract, ExtractNoMatch, InjectHosts, InjectScope, MaskDuplicates, ResponseHeaderStrip,
    SecretMode, SecretName, SecretTarget,
};

/// The JSON shape of a [`SecretBinding`](crate::SecretBinding) before its rules are checked.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct SecretBindingWire {
    pub(crate) name: SecretName,
    pub(crate) target: SecretTarget,
    pub(crate) mode: SecretMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) extract: Option<Extract>,
    #[serde(default, skip_serializing_if = "MaskDuplicates::is_default")]
    pub(crate) mask_duplicates: MaskDuplicates,
    #[serde(default, skip_serializing_if = "ExtractNoMatch::is_default")]
    pub(crate) on_extract_no_match: ExtractNoMatch,
    #[serde(default, skip_serializing_if = "ResponseHeaderStrip::is_empty")]
    pub(crate) strip_response_headers: ResponseHeaderStrip,
    pub(crate) inject_hosts: InjectHosts,
    #[serde(default, flatten, skip_serializing_if = "InjectScope::is_default")]
    pub(crate) inject_scope: InjectScope,
    pub(crate) source: CredentialRequest,
}
