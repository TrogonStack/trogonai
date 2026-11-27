/// Why a configured root was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RootPolicyError {
    #[error("root {0:?} must be an absolute sandbox path")]
    NotAbsolute(String),
    #[error("root {0:?} must not contain `..`")]
    ParentComponent(String),
}
