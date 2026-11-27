use crate::SecretName;

/// A set of fields is not a secret binding [`SecretBinding`](crate::SecretBinding) accepts.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum SecretBindingError {
    #[error("secret binding '{0}' has mode deny, which does not take an extract pattern")]
    ExtractOnDeny(SecretName),
    #[error("secret binding '{0}' targets an environment variable, which takes only a regex or jwt extract rule")]
    ExtractOnEnv(SecretName),
    #[error("secret binding '{0}' sets on_extract_no_match without an extract rule")]
    NoMatchPolicyWithoutExtract(SecretName),
    #[error("secret binding '{0}' has mode deny, which does not strip response headers")]
    StripOnDeny(SecretName),
    #[error("secret binding '{0}' has mode deny, which takes no path_prefix or methods")]
    ScopeOnDeny(SecretName),
}
