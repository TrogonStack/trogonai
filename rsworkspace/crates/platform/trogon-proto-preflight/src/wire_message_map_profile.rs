#[derive(Clone, Debug)]
pub struct WireMessageMapProfile;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WireMessageMapProfileError {
    #[error("message map profile is invalid")]
    Invalid,
}
