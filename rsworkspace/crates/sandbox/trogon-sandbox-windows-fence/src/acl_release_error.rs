/// What stayed behind when a holder's access entries could not be released.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AclReleaseError {
    #[error("the session grants were not all released, so the denies stay (pending {pending:?}, {detail})")]
    GrantsNotReleased { pending: Vec<String>, detail: String },
    #[error("the session denies could not be restored")]
    DeniesNotRestored,
}
