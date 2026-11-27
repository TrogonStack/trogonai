use crate::SecretName;

/// A list of pairs does not fit the bindings it links, so [`AwsCredentialPairs`](crate::AwsCredentialPairs) refuses
/// it.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum AwsCredentialPairsError {
    #[error("AWS credential pair names unknown secret binding '{0}'")]
    UnknownBinding(SecretName),
    #[error("AWS credential pair member '{0}' must be a whole-value mask binding")]
    MemberNotWholeMask(SecretName),
    #[error("AWS credential pair member '{0}' appears more than once")]
    DuplicateMember(SecretName),
    #[error("AWS credential pair members '{access_key_id}' and '{member}' inject at different hosts")]
    HostsDiffer {
        access_key_id: SecretName,
        member: SecretName,
    },
}
