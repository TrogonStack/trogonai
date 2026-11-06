#[derive(Debug, thiserror::Error)]
pub enum RegistrationError {
    #[error("registration authority refused")]
    Refused,
}
