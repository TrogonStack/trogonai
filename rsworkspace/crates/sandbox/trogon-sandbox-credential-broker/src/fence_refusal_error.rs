/// Why a request was refused by the policy fence.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum FenceRefusalError {
    #[error("a policy fence is required")]
    Missing,
    #[error("credential reference is not named by the committed policy")]
    NotCommitted,
    #[error("the policy store is unavailable")]
    Unavailable,
}
