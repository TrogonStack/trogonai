/// A Modal App name that cannot name an App.
#[derive(Debug, thiserror::Error)]
pub enum ModalAppNameError {
    #[error("the Modal App name is empty")]
    Empty,
    #[error("the Modal App name has whitespace or control characters")]
    Unsafe,
}
