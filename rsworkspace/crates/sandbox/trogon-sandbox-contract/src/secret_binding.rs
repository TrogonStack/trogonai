use serde::{Deserialize, Serialize};

use crate::secret_binding_wire::SecretBindingWire;
use crate::{
    CredentialRequest, Extract, ExtractNoMatch, InjectHosts, InjectScope, MaskDuplicates, ResponseHeaderStrip,
    SecretBindingError, SecretBindingParts, SecretMode, SecretName, SecretTarget,
};

/// One credential the broker holds and the sandbox must never see in the clear.
///
/// The real value is fetched through the credential request at runtime and is never part of this type. A binding
/// accepts only a coherent set of fields. A `deny` binding takes no extract rule, no response header stripping, and no
/// path or method scope, because nothing is ever injected. An environment variable target takes only a regex or jwt
/// extract rule. A no-match policy needs an extract rule to apply to. Construction and deserialization both enforce
/// these rules.
///
/// The scope fields serialize flat next to the others, and every defaulted field is omitted from the wire.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "SecretBindingWire", into = "SecretBindingWire")]
pub struct SecretBinding {
    name: SecretName,
    target: SecretTarget,
    mode: SecretMode,
    extract: Option<Extract>,
    mask_duplicates: MaskDuplicates,
    on_extract_no_match: ExtractNoMatch,
    strip_response_headers: ResponseHeaderStrip,
    inject_hosts: InjectHosts,
    inject_scope: InjectScope,
    source: CredentialRequest,
}

impl SecretBinding {
    pub fn new(parts: SecretBindingParts) -> Result<Self, SecretBindingError> {
        let name = &parts.name;
        if parts.mode == SecretMode::Deny {
            if parts.extract.is_some() {
                return Err(SecretBindingError::ExtractOnDeny(name.clone()));
            }
            if !parts.strip_response_headers.is_empty() {
                return Err(SecretBindingError::StripOnDeny(name.clone()));
            }
            if !parts.inject_scope.is_request_independent() {
                return Err(SecretBindingError::ScopeOnDeny(name.clone()));
            }
        }
        if let (SecretTarget::Env(_), Some(extract)) = (&parts.target, &parts.extract)
            && !extract.fits_env_target()
        {
            return Err(SecretBindingError::ExtractOnEnv(name.clone()));
        }
        if !parts.on_extract_no_match.is_default() && parts.extract.is_none() {
            return Err(SecretBindingError::NoMatchPolicyWithoutExtract(name.clone()));
        }
        Ok(Self {
            name: parts.name,
            target: parts.target,
            mode: parts.mode,
            extract: parts.extract,
            mask_duplicates: parts.mask_duplicates,
            on_extract_no_match: parts.on_extract_no_match,
            strip_response_headers: parts.strip_response_headers,
            inject_hosts: parts.inject_hosts,
            inject_scope: parts.inject_scope,
            source: parts.source,
        })
    }

    pub fn name(&self) -> &SecretName {
        &self.name
    }

    pub fn target(&self) -> &SecretTarget {
        &self.target
    }

    pub fn mode(&self) -> SecretMode {
        self.mode
    }

    pub fn extract(&self) -> Option<&Extract> {
        self.extract.as_ref()
    }

    pub fn mask_duplicates(&self) -> MaskDuplicates {
        self.mask_duplicates
    }

    pub fn on_extract_no_match(&self) -> ExtractNoMatch {
        self.on_extract_no_match
    }

    /// Response headers removed from replies of the inject hosts before the sandbox sees them.
    pub fn strip_response_headers(&self) -> &ResponseHeaderStrip {
        &self.strip_response_headers
    }

    pub fn inject_hosts(&self) -> &InjectHosts {
        &self.inject_hosts
    }

    /// The path prefix, method set, and port meaning that narrow where the inject hosts may receive the credential.
    pub fn inject_scope(&self) -> &InjectScope {
        &self.inject_scope
    }

    pub fn source(&self) -> &CredentialRequest {
        &self.source
    }

    /// Reports whether the binding is a whole-value mask: it masks the credential and has no extract rule.
    pub fn is_whole_value_mask(&self) -> bool {
        self.mode == SecretMode::Mask && self.extract.is_none()
    }
}

impl TryFrom<SecretBindingWire> for SecretBinding {
    type Error = SecretBindingError;

    fn try_from(wire: SecretBindingWire) -> Result<Self, Self::Error> {
        Self::new(SecretBindingParts {
            name: wire.name,
            target: wire.target,
            mode: wire.mode,
            extract: wire.extract,
            mask_duplicates: wire.mask_duplicates,
            on_extract_no_match: wire.on_extract_no_match,
            strip_response_headers: wire.strip_response_headers,
            inject_hosts: wire.inject_hosts,
            inject_scope: wire.inject_scope,
            source: wire.source,
        })
    }
}

impl From<SecretBinding> for SecretBindingWire {
    fn from(binding: SecretBinding) -> Self {
        Self {
            name: binding.name,
            target: binding.target,
            mode: binding.mode,
            extract: binding.extract,
            mask_duplicates: binding.mask_duplicates,
            on_extract_no_match: binding.on_extract_no_match,
            strip_response_headers: binding.strip_response_headers,
            inject_hosts: binding.inject_hosts,
            inject_scope: binding.inject_scope,
            source: binding.source,
        }
    }
}
