use crate::SecretName;

/// A set of member names is not an AWS credential pair [`AwsCredentialPairBinding`](crate::AwsCredentialPairBinding)
/// accepts.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum AwsCredentialPairBindingError {
    #[error("AWS credential pair member '{0}' appears more than once")]
    DuplicateMember(SecretName),
}
