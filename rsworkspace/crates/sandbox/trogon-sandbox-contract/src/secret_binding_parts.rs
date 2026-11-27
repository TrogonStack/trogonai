use crate::{
    CredentialRequest, Extract, ExtractNoMatch, InjectHosts, InjectScope, MaskDuplicates, ResponseHeaderStrip,
    SecretMode, SecretName, SecretTarget,
};

/// The fields of a [`SecretBinding`](crate::SecretBinding) before its rules are checked.
///
/// Hand them to [`SecretBinding::new`](crate::SecretBinding::new).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SecretBindingParts {
    pub name: SecretName,
    pub target: SecretTarget,
    pub mode: SecretMode,
    pub extract: Option<Extract>,
    pub mask_duplicates: MaskDuplicates,
    pub on_extract_no_match: ExtractNoMatch,
    pub strip_response_headers: ResponseHeaderStrip,
    pub inject_hosts: InjectHosts,
    pub inject_scope: InjectScope,
    pub source: CredentialRequest,
}
