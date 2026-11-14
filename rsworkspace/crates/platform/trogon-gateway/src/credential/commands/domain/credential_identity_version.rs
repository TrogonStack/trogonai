/// The rule a credential id was derived by.
///
/// The write request records the rule beside the id it minted. Replay reads that
/// id back and never derives it again, so a new rule is a new version here and the
/// ids already recorded keep the rule they were minted under.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CredentialIdentityVersion {
    /// `{owner_id}/{scope_key}/{kind}`, hashed under the credential id namespace.
    V1,
}
